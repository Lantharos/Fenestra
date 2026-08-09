#include "osr/screen.h"

#include <cstdlib>

#include "osr/handler.h"
#include "include/wrapper/cef_helpers.h"

namespace sabine_osr {

bool TryHandleScreenOriginControl(SabineOsrHandler* handler,
                                  const std::vector<std::string>& parts) {
  if (!handler || parts.empty() || parts[0] != "screen_origin") {
    return false;
  }
  CEF_REQUIRE_UI_THREAD();
  if (parts.size() < 3) {
    return true;
  }
  handler->SetScreenOrigin(std::atoi(parts[1].c_str()),
                           std::atoi(parts[2].c_str()));
  return true;
}

}  // namespace sabine_osr

bool SabineOsrHandler::GetScreenPoint(CefRefPtr<CefBrowser> browser,
                                      int viewX,
                                      int viewY,
                                      int& screenX,
                                      int& screenY) {
  (void)browser;
  screenX = screen_origin_x_ + viewX;
  screenY = screen_origin_y_ + viewY;
  return true;
}

void SabineOsrHandler::SetScreenOrigin(int x, int y) {
  screen_origin_x_ = x;
  screen_origin_y_ = y;
  if (browser_) {
    browser_->GetHost()->NotifyScreenInfoChanged();
  }
  NotifyGuestScreenInfo();
}
