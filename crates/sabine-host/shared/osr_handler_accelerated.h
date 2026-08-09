#ifndef SABINE_CEF_HOST_OSR_HANDLER_ACCELERATED_H_
#define SABINE_CEF_HOST_OSR_HANDLER_ACCELERATED_H_

#include "include/cef_command_line.h"
#include "include/internal/cef_types.h"

namespace sabine_osr {

bool PreferSharedTexture(CefRefPtr<CefCommandLine> command_line);
void ApplySharedTexture(CefWindowInfo* window_info, bool enabled);

}  // namespace sabine_osr

#endif
