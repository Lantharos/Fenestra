#ifndef SABINE_CEF_HOST_OSR_HANDLER_SCREEN_H_
#define SABINE_CEF_HOST_OSR_HANDLER_SCREEN_H_

#include <string>
#include <vector>

class SabineOsrHandler;

namespace sabine_osr {

// Returns true when |parts| is a `screen_origin` control line and was handled.
bool TryHandleScreenOriginControl(SabineOsrHandler* handler,
                                  const std::vector<std::string>& parts);

}  // namespace sabine_osr

#endif
