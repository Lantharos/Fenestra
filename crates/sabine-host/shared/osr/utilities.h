#ifndef SABINE_CEF_HOST_OSR_UTILITIES_H_
#define SABINE_CEF_HOST_OSR_UTILITIES_H_

#include <cstdint>
#include <mutex>
#include <set>
#include <string>
#include <vector>

#ifdef _WIN32
#include <BaseTsd.h>
typedef SSIZE_T ssize_t;
typedef long off_t;
#else
#include <sys/types.h>
#endif

#include "include/cef_command_line.h"
#include "include/internal/cef_types.h"

class SabineOsrHandler;

namespace sabine_osr {

extern SabineOsrHandler* g_instance;
extern std::mutex g_handlers_mutex;
extern std::vector<SabineOsrHandler*> g_handlers;
extern const size_t kSharedPaintThreshold;
extern const size_t kBatchEntryLen;

struct PaintRectBytes {
  int x = 0;
  int y = 0;
  int width = 0;
  int height = 0;
  uint64_t offset = 0;
  uint32_t len = 0;
};

void RegisterHandler(SabineOsrHandler* handler);
void UnregisterHandler(SabineOsrHandler* handler);
bool HasRegisteredHandlers();
std::vector<SabineOsrHandler*> SnapshotHandlers();

int SwitchInt(CefRefPtr<CefCommandLine> command_line,
              const std::string& name,
              int fallback);
float SwitchFloat(CefRefPtr<CefCommandLine> command_line,
                  const std::string& name,
                  float fallback);
std::vector<std::string> Split(const std::string& value, char separator);
std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line);
std::string DecodeUriComponent(const std::string& value);
std::string QueryValue(const std::string& url, const std::string& name);
std::string BridgeRequestId(const std::string& url);
std::string UrlOrigin(const std::string& url);
std::string HtmlEscape(const std::string& value);
std::string BridgeInstallScript(const std::set<std::string>& commands);
bool ParseBridgeResponse(const std::string& line,
                         std::string* browser_id,
                         std::string* request_id,
                         bool* ok,
                         std::string* payload);
bool ParseBridgeEvent(const std::string& line,
                      std::string* name_json,
                      std::string* payload);
bool ParseHostControl(const std::string& line,
                      std::string* command,
                      std::string* value);

void PutU32(std::vector<char>* buffer, size_t offset, uint32_t value);
void PutI32(std::vector<char>* buffer, size_t offset, int32_t value);
void PutU64(std::vector<char>* buffer, size_t offset, uint64_t value);
bool SendAll(intptr_t fd, const char* bytes, size_t len);
#ifndef _WIN32
int CreateMemfd(const char* name);
bool WriteAllAt(int fd, const char* bytes, size_t len, off_t offset);
#endif
void PutPaintEntry(std::vector<char>* payload,
                   size_t offset,
                   const PaintRectBytes& rect);
bool CopyPaintRect(char* destination,
                   const void* buffer,
                   int buffer_width,
                   const PaintRectBytes& rect);
bool WritePaintRect(int fd,
                    const void* buffer,
                    int buffer_width,
                    const PaintRectBytes& rect);
uint32_t BatchKind(uint32_t frame_kind);
uint32_t SharedBatchKind(uint32_t frame_kind);

int KeyCodeForName(const std::string& key);
std::u16string Utf8ToUtf16(const std::string& value);
cef_mouse_button_type_t MouseButtonFromString(const std::string& value);
std::string CursorName(cef_cursor_type_t type);

}  // namespace sabine_osr

#endif
