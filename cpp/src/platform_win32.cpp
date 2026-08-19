// SPDX-License-Identifier: Apache-2.0
//
// Win32 backend for the platform layer. Exactly one of this file and
// platform_posix.cpp is compiled into any given binary.
//
// This is one of the two files permitted to include OS headers. See
// scripts/assert-platform-confined.sh.
//
// Windows is a shipping target, not a development convenience, so this backend
// is held to behavioural parity with the POSIX one rather than to "it compiles
// and mostly works":
//
//   * Paths are widened to UTF-16 and the W entry points are called, so a
//     non-ASCII path behaves as it does under the Rust implementation instead
//     of being mangled by the active ANSI codepage.
//   * The commit uses MoveFileExW(MOVEFILE_REPLACE_EXISTING), because the CRT's
//     rename() FAILS when the destination exists. Rust's std::fs::rename uses
//     MoveFileEx internally, so a port written with rename() would be green on
//     Linux and silently wrong on the shipped Windows binary.
//   * Handles are closed before the rename: NTFS refuses to rename a file that
//     still has an open handle, where POSIX does not care.
//   * The input is opened with all three FILE_SHARE bits so decoding never
//     locks a recording another tool is reading.

#include "mie/platform.hpp"

#if !defined(_WIN32)
#error "platform_win32.cpp must not be compiled for non-Windows targets"
#endif

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>
// <windows.h> must precede these.
#include <fcntl.h>
#include <io.h>
#include <stdio.h>

namespace mie {
namespace platform {

namespace {

/// NULL, not INVALID_HANDLE_VALUE, is this layer's "closed" sentinel.
///
/// The two differ: CreateFile reports failure as INVALID_HANDLE_VALUE ((HANDLE)-1)
/// while CreateFileMapping reports it as NULL. Normalising to NULL at every
/// failure site means the destructors have exactly one value to test, instead of
/// each one having to remember which API it came from -- which is a classic way
/// to close a handle of -1 or leak a real one.
HANDLE as_handle(void* p) { return static_cast<HANDLE>(p); }

void fill_last_error(OsError& err, DWORD captured) {
    err.code = static_cast<int>(captured);
    err.message.clear();

    LPWSTR buffer = 0;
    const DWORD flags =
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
    const DWORD length =
        ::FormatMessageW(flags, 0, captured, MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT),
                         reinterpret_cast<LPWSTR>(&buffer), 0, 0);
    if (length != 0 && buffer != 0) {
        std::wstring wide(buffer, length);
        // FormatMessage terminates its text with CRLF. Leaving it in would put
        // a line break in the middle of a diagnostic that the logger then
        // prefixes -- one message across two lines, only on Windows.
        while (!wide.empty() &&
               (wide[wide.size() - 1] == L'\r' || wide[wide.size() - 1] == L'\n')) {
            wide.erase(wide.size() - 1);
        }
        err.message = from_wide(wide);
    }
    if (buffer != 0) {
        ::LocalFree(buffer);
    }
    if (err.message.empty()) {
        err.message = "Windows error";
    }
}

void fill_last_error(OsError& err) { fill_last_error(err, ::GetLastError()); }

/// Strip the \\?\ extended-length prefix GetFinalPathNameByHandleW returns.
///
/// Both sides of a comparison go through this function, so keeping the prefix
/// would still compare correctly -- it is removed because the canonical path
/// also reaches operators in error messages, and "\\?\C:\logs\out.csv" reads
/// like a corrupted path to someone who has not met that prefix before.
std::wstring strip_extended_prefix(const std::wstring& path) {
    static const wchar_t kUnc[] = L"\\\\?\\UNC\\";
    static const wchar_t kDos[] = L"\\\\?\\";
    if (path.compare(0, 8, kUnc) == 0) {
        return std::wstring(L"\\\\") + path.substr(8);
    }
    if (path.compare(0, 4, kDos) == 0) {
        return path.substr(4);
    }
    return path;
}

}  // namespace

// ---------------------------------------------------------------------------
// MappedFile
// ---------------------------------------------------------------------------

MappedFile::MappedFile() : file_handle_(0), mapping_handle_(0), data_(0), size_(0) {}

MappedFile::~MappedFile() { close(); }

MappedFile::MappedFile(MappedFile&& other) noexcept
    : file_handle_(other.file_handle_),
      mapping_handle_(other.mapping_handle_),
      data_(other.data_),
      size_(other.size_) {
    other.file_handle_ = 0;
    other.mapping_handle_ = 0;
    other.data_ = 0;
    other.size_ = 0;
}

MappedFile& MappedFile::operator=(MappedFile&& other) noexcept {
    if (this != &other) {
        close();
        file_handle_ = other.file_handle_;
        mapping_handle_ = other.mapping_handle_;
        data_ = other.data_;
        size_ = other.size_;
        other.file_handle_ = 0;
        other.mapping_handle_ = 0;
        other.data_ = 0;
        other.size_ = 0;
    }
    return *this;
}

bool MappedFile::open(const std::string& utf8_path, OsError& err) {
    close();
    err.clear();

    const std::wstring wide = to_wide(utf8_path);

    // Open BEFORE testing the size, deliberately, and matching the POSIX
    // backend: a directory reports a zero size here, so a size-first check
    // would report "empty" for a path that is really the wrong kind of thing.
    const HANDLE file = ::CreateFileW(wide.c_str(), GENERIC_READ,
                                      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 0,
                                      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, 0);
    if (file == INVALID_HANDLE_VALUE) {
        fill_last_error(err);
        return false;
    }

    BY_HANDLE_FILE_INFORMATION info;
    if (::GetFileInformationByHandle(file, &info) == 0) {
        fill_last_error(err);
        ::CloseHandle(file);
        return false;
    }
    if ((info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0) {
        err.code = ERROR_DIRECTORY_NOT_SUPPORTED;
        err.message = "not a regular file";
        ::CloseHandle(file);
        return false;
    }

    LARGE_INTEGER file_size;
    if (::GetFileSizeEx(file, &file_size) == 0) {
        fill_last_error(err);
        ::CloseHandle(file);
        return false;
    }
    if (file_size.QuadPart == 0) {
        // Callers reject empty inputs as FileEmpty before reaching here
        // (L2-RDR-006). Guarded anyway because CreateFileMapping refuses a
        // zero-length file outright, which would surface as a confusing
        // mapping failure rather than the specific error the operator needs.
        err.code = ERROR_HANDLE_EOF;
        err.message = "file is empty";
        ::CloseHandle(file);
        return false;
    }

    const HANDLE mapping = ::CreateFileMappingW(file, 0, PAGE_READONLY, 0, 0, 0);
    if (mapping == 0) {
        fill_last_error(err);
        ::CloseHandle(file);
        return false;
    }

    LPVOID view = ::MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
    if (view == 0) {
        fill_last_error(err);
        ::CloseHandle(mapping);
        ::CloseHandle(file);
        return false;
    }

    file_handle_ = file;
    mapping_handle_ = mapping;
    data_ = static_cast<const uint8_t*>(view);
    size_ = static_cast<uint64_t>(file_size.QuadPart);
    return true;
}

void MappedFile::close() {
    if (data_ != 0) {
        ::UnmapViewOfFile(static_cast<LPCVOID>(data_));
        data_ = 0;
        size_ = 0;
    }
    if (mapping_handle_ != 0) {
        ::CloseHandle(as_handle(mapping_handle_));
        mapping_handle_ = 0;
    }
    if (file_handle_ != 0) {
        ::CloseHandle(as_handle(file_handle_));
        file_handle_ = 0;
    }
}

// ---------------------------------------------------------------------------
// AtomicFile
// ---------------------------------------------------------------------------

AtomicFile::AtomicFile() : handle_(0), temp_path_(), final_path_(), buffer_(), committed_(false) {
    buffer_.reserve(kWriteBufferSize);
}

AtomicFile::~AtomicFile() { abort(); }

bool AtomicFile::create(const std::string& final_utf8_path, OsError& err) {
    abort();
    err.clear();
    committed_ = false;
    final_path_ = final_utf8_path;

    for (int attempt = 0; attempt < 8; ++attempt) {
        const std::string candidate = make_temp_name(final_utf8_path);
        const std::wstring wide = to_wide(candidate);
        // FILE_SHARE_READ | FILE_SHARE_DELETE, not 0.
        //
        // Share mode 0 denies every other opener, which makes the in-progress
        // temp file unopenable even for reading. That is a behavioural
        // divergence from both of the other implementations -- POSIX places no
        // such lock, and Rust's File::create_new shares read, write and delete
        // -- and it is not merely cosmetic: an antivirus or backup agent that
        // touches the file mid-write turns into a spurious decode failure.
        //
        // FILE_SHARE_WRITE is deliberately withheld. Readers are harmless;
        // a second writer into the temp file would corrupt the CSV being
        // streamed into it. FILE_SHARE_DELETE is granted so cleanup can unlink
        // the file even while a scanner holds it open.
        const HANDLE file =
            ::CreateFileW(wide.c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_DELETE, 0,
                          CREATE_NEW, FILE_ATTRIBUTE_NORMAL, 0);
        if (file != INVALID_HANDLE_VALUE) {
            handle_ = file;
            temp_path_ = candidate;
            buffer_.clear();
            return true;
        }
        const DWORD captured = ::GetLastError();
        if (captured != ERROR_FILE_EXISTS && captured != ERROR_ALREADY_EXISTS) {
            fill_last_error(err, captured);
            return false;
        }
    }
    err.code = ERROR_FILE_EXISTS;
    err.message = "could not create a unique temporary file beside the destination";
    return false;
}

bool AtomicFile::raw_write(const char* bytes, std::size_t len, OsError& err) {
    if (handle_ == 0) {
        err.code = ERROR_INVALID_HANDLE;
        err.message = "temporary file is not open";
        return false;
    }
    std::size_t written = 0;
    while (written < len) {
        const std::size_t remaining = len - written;
        // WriteFile counts in DWORD. A single call therefore cannot exceed 4 GiB,
        // and the loop below already handles a short write, so clamping here is
        // free rather than a special case.
        const DWORD chunk = remaining > 0x40000000u ? 0x40000000u : static_cast<DWORD>(remaining);
        DWORD produced = 0;
        if (::WriteFile(as_handle(handle_), bytes + written, chunk, &produced, 0) == 0) {
            fill_last_error(err);
            return false;
        }
        if (produced == 0) {
            err.code = ERROR_WRITE_FAULT;
            err.message = "write made no progress";
            return false;
        }
        written += produced;
    }
    return true;
}

bool AtomicFile::flush(OsError& err) {
    if (buffer_.empty()) {
        return true;
    }
    if (!raw_write(&buffer_[0], buffer_.size(), err)) {
        return false;
    }
    buffer_.clear();
    return true;
}

bool AtomicFile::write(const char* bytes, std::size_t len, OsError& err) {
    err.clear();
    if (len >= kWriteBufferSize) {
        return flush(err) && raw_write(bytes, len, err);
    }
    if (buffer_.size() + len > kWriteBufferSize && !flush(err)) {
        return false;
    }
    buffer_.insert(buffer_.end(), bytes, bytes + len);
    return true;
}

bool AtomicFile::commit(OsError& err) { return commit_with_suffix(std::string(), err); }

bool AtomicFile::commit_with_suffix(const std::string& suffix, OsError& err) {
    err.clear();
    if (!flush(err)) {
        return false;
    }
    if (handle_ == 0) {
        err.code = ERROR_INVALID_HANDLE;
        err.message = "temporary file is not open";
        return false;
    }

    // Close before renaming: NTFS refuses to rename a file with an open handle.
    // POSIX has no such rule, which is why this ordering has to be deliberate
    // rather than incidental -- code that renames first is correct on Linux and
    // fails on every Windows run.
    if (::CloseHandle(as_handle(handle_)) == 0) {
        fill_last_error(err);
        handle_ = 0;
        return false;
    }
    handle_ = 0;

    const std::string destination = final_path_ + suffix;
    const std::wstring wide_temp = to_wide(temp_path_);
    const std::wstring wide_dest = to_wide(destination);

    // MOVEFILE_REPLACE_EXISTING is the whole reason this is not std::rename.
    if (::MoveFileExW(wide_temp.c_str(), wide_dest.c_str(), MOVEFILE_REPLACE_EXISTING) == 0) {
        fill_last_error(err);
        return false;
    }
    committed_ = true;
    temp_path_.clear();
    return true;
}

void AtomicFile::abort() {
    if (handle_ != 0) {
        ::CloseHandle(as_handle(handle_));
        handle_ = 0;
    }
    if (!committed_ && !temp_path_.empty()) {
        ::DeleteFileW(to_wide(temp_path_).c_str());
        temp_path_.clear();
    }
    buffer_.clear();
}

// ---------------------------------------------------------------------------
// Directory enumeration, metadata, identity
// ---------------------------------------------------------------------------

bool list_directory(const std::string& utf8_dir, std::vector<std::string>& names, OsError& err) {
    err.clear();
    names.clear();

    const std::wstring pattern = to_wide(path_join(utf8_dir, std::string("*")));

    WIN32_FIND_DATAW entry;
    const HANDLE search = ::FindFirstFileW(pattern.c_str(), &entry);
    if (search == INVALID_HANDLE_VALUE) {
        const DWORD captured = ::GetLastError();
        if (captured == ERROR_FILE_NOT_FOUND) {
            // An empty directory is not an error; it is a directory with no
            // matches. Reporting it as failure would turn a --glob that matched
            // nothing into a crash instead of an empty result.
            return true;
        }
        fill_last_error(err, captured);
        return false;
    }

    do {
        const std::wstring name(entry.cFileName);
        if (name == L"." || name == L"..") {
            continue;
        }
        names.push_back(from_wide(name));
    } while (::FindNextFileW(search, &entry) != 0);

    const DWORD captured = ::GetLastError();
    ::FindClose(search);

    if (captured != ERROR_NO_MORE_FILES) {
        fill_last_error(err, captured);
        names.clear();
        return false;
    }
    return true;
}

void set_stdout_binary() {
    // Without this the CRT rewrites every newline into CRLF on the way out,
    // which breaks every stdout conformance oracle on Windows alone while the
    // file-destination oracles keep passing -- a split that is very hard to
    // read as "one missing call".
    ::_setmode(::_fileno(stdout), _O_BINARY);
}

bool canonical_path(const std::string& utf8_path, std::string& out, OsError& err) {
    err.clear();
    out.clear();

    const std::wstring wide = to_wide(utf8_path);
    // FILE_FLAG_BACKUP_SEMANTICS is required to open a DIRECTORY handle, and
    // this function is called on directories -- paths_same_file resolves the
    // output's parent when the output itself does not exist yet.
    const HANDLE handle = ::CreateFileW(wide.c_str(), FILE_READ_ATTRIBUTES,
                                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 0,
                                        OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, 0);
    if (handle == INVALID_HANDLE_VALUE) {
        fill_last_error(err);
        return false;
    }

    DWORD needed = ::GetFinalPathNameByHandleW(handle, 0, 0, VOLUME_NAME_DOS);
    if (needed == 0) {
        fill_last_error(err);
        ::CloseHandle(handle);
        return false;
    }

    std::vector<wchar_t> buffer(needed + 1, L'\0');
    const DWORD written = ::GetFinalPathNameByHandleW(handle, &buffer[0], needed, VOLUME_NAME_DOS);
    ::CloseHandle(handle);

    if (written == 0 || written >= needed + 1) {
        fill_last_error(err);
        return false;
    }

    out = from_wide(strip_extended_prefix(std::wstring(&buffer[0], written)));
    return true;
}

bool file_metadata(const std::string& utf8_path, uint64_t& size, bool& is_regular, OsError& err) {
    err.clear();
    size = 0;
    is_regular = false;

    WIN32_FILE_ATTRIBUTE_DATA data;
    if (::GetFileAttributesExW(to_wide(utf8_path).c_str(), GetFileExInfoStandard, &data) == 0) {
        fill_last_error(err);
        return false;
    }

    ULARGE_INTEGER combined;
    combined.HighPart = data.nFileSizeHigh;
    combined.LowPart = data.nFileSizeLow;
    size = static_cast<uint64_t>(combined.QuadPart);
    is_regular = (data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
    return true;
}

bool remove_file(const std::string& utf8_path, OsError& err) {
    err.clear();
    if (::DeleteFileW(to_wide(utf8_path).c_str()) == 0) {
        fill_last_error(err);
        return false;
    }
    return true;
}

uint64_t process_id() { return static_cast<uint64_t>(::GetCurrentProcessId()); }

// ---------------------------------------------------------------------------
// UTF-8 <-> UTF-16
// ---------------------------------------------------------------------------
//
// Real conversions here, unlike the POSIX backend's byte pass-through. This is
// what makes a non-ASCII path work rather than being mangled by the active ANSI
// codepage, and it is why Windows can be a shipping target rather than a
// development convenience.

std::wstring to_wide(const std::string& utf8) {
    if (utf8.empty()) {
        return std::wstring();
    }
    const int source_len = static_cast<int>(utf8.size());
    const int needed = ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), source_len, 0, 0);
    if (needed <= 0) {
        return std::wstring();
    }
    std::wstring out(static_cast<std::size_t>(needed), L'\0');
    ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), source_len, &out[0], needed);
    return out;
}

std::string from_wide(const std::wstring& wide) {
    if (wide.empty()) {
        return std::string();
    }
    const int source_len = static_cast<int>(wide.size());
    const int needed = ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), source_len, 0, 0, 0, 0);
    if (needed <= 0) {
        return std::string();
    }
    std::string out(static_cast<std::size_t>(needed), '\0');
    ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), source_len, &out[0], needed, 0, 0);
    return out;
}

}  // namespace platform
}  // namespace mie
