#include "osr_handler_ime.h"

#include <cstdlib>

#include "include/internal/cef_types.h"
#include "osr_handler_util.h"

namespace sabine_osr {

bool TryHandleImeControl(CefRefPtr<CefBrowserHost> host,
                         const std::vector<std::string>& parts) {
  if (!host || parts.empty()) {
    return false;
  }
  if (parts[0] == "ime_cancel") {
    host->ImeCancelComposition();
    return true;
  }
  if (parts[0] == "ime_finish") {
    const bool keep_selection =
        parts.size() >= 2 && parts[1] == "1";
    host->ImeFinishComposingText(keep_selection);
    return true;
  }
  if (parts[0] == "ime_commit" && parts.size() >= 2) {
    CefString text = DecodeUriComponent(parts[1]);
    host->ImeCommitText(text, CefRange(UINT32_MAX, UINT32_MAX), 0);
    return true;
  }
  if (parts[0] == "ime_composition" && parts.size() >= 2) {
    CefString text = DecodeUriComponent(parts[1]);
    std::vector<CefCompositionUnderline> underlines;
    host->ImeSetComposition(text, underlines, CefRange(UINT32_MAX, UINT32_MAX),
                            CefRange(UINT32_MAX, UINT32_MAX));
    return true;
  }
  return false;
}

}  // namespace sabine_osr
