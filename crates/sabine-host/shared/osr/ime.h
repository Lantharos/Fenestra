#ifndef SABINE_CEF_HOST_OSR_IME_H_
#define SABINE_CEF_HOST_OSR_IME_H_

#include <string>
#include <vector>

#include "include/cef_browser.h"

namespace sabine_osr {

// Returns true when |parts| is an IME control line and was handled.
bool TryHandleImeControl(CefRefPtr<CefBrowserHost> host,
                         const std::vector<std::string>& parts);

}  // namespace sabine_osr

#endif
