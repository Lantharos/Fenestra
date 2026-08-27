#include "osr/ime.h"

#include <algorithm>
#include <cstdlib>

#include "include/internal/cef_types.h"
#include "include/wrapper/cef_helpers.h"
#include "osr/handler.h"
#include "osr/utilities.h"

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
    const uint32_t length = static_cast<uint32_t>(text.length());
    if (length > 0) {
      cef_composition_underline_t underline = {
          sizeof(cef_composition_underline_t), CefRange(0, length),
          0xFF000000, 0, false};
      underlines.push_back(underline);
    }
    CefRange selection(UINT32_MAX, UINT32_MAX);
    if (parts.size() >= 4) {
      const uint32_t start =
          static_cast<uint32_t>(std::strtoul(parts[2].c_str(), nullptr, 10));
      const uint32_t end =
          static_cast<uint32_t>(std::strtoul(parts[3].c_str(), nullptr, 10));
      if (start <= end && end <= length) {
        selection = CefRange(start, end);
      }
    }
    host->ImeSetComposition(text, underlines, CefRange(UINT32_MAX, UINT32_MAX),
                            selection);
    return true;
  }
  return false;
}

}  // namespace sabine_osr

void SabineOsrHandler::OnImeCompositionRangeChanged(
    CefRefPtr<CefBrowser> browser,
    const CefRange& selected_range,
    const RectList& character_bounds) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser || character_bounds.empty()) {
    return;
  }
  const size_t caret =
      std::min<size_t>(selected_range.to, character_bounds.size());
  CefRect rect;
  if (caret == character_bounds.size()) {
    rect = character_bounds.back();
    rect.x += rect.width;
    rect.width = 1;
  } else {
    rect = character_bounds[caret];
    rect.width = std::max(1, rect.width);
  }
  rect.height = std::max(1, rect.height);
  ime_cursor_rects_[browser->GetIdentifier()] = rect;
  SendFocusedImeState();
}

void SabineOsrHandler::OnVirtualKeyboardRequested(
    CefRefPtr<CefBrowser> browser,
    TextInputMode input_mode) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser) {
    return;
  }
  text_input_modes_[browser->GetIdentifier()] = input_mode;
  SendFocusedImeState();
}

void SabineOsrHandler::SendFocusedImeState() {
  CEF_REQUIRE_UI_THREAD();
  CefRefPtr<CefBrowser> target = browser_;
  int offset_x = 0;
  int offset_y = 0;
  if (!focused_guest_id_.empty()) {
    GuestView* guest = guests_.Find(focused_guest_id_);
    if (!guest || !guest->browser) {
      SendMessage(kImeStateChanged, CEF_TEXT_INPUT_MODE_NONE, 0, 0, 0,
                  nullptr, 0);
      return;
    }
    target = guest->browser;
    offset_x = guest->bounds.x;
    offset_y = guest->bounds.y;
  }
  if (!target) {
    SendMessage(kImeStateChanged, CEF_TEXT_INPUT_MODE_NONE, 0, 0, 0, nullptr,
                0);
    return;
  }
  const int browser_id = target->GetIdentifier();
  const auto mode = text_input_modes_.find(browser_id);
  const cef_text_input_mode_t input_mode =
      mode == text_input_modes_.end() ? CEF_TEXT_INPUT_MODE_NONE : mode->second;
  SendMessage(kImeStateChanged, static_cast<uint32_t>(input_mode), 0, 0, 0,
              nullptr, 0);
  if (input_mode == CEF_TEXT_INPUT_MODE_NONE) {
    return;
  }
  const auto surrounding = ime_surrounding_state_.find(browser_id);
  if (surrounding != ime_surrounding_state_.end()) {
    const std::string& payload = surrounding->second;
    SendMessage(kImeSurroundingChanged, 0, 0, 0, 0, payload.data(),
                static_cast<uint32_t>(payload.size()));
  }
  const auto cursor = ime_cursor_rects_.find(browser_id);
  if (cursor == ime_cursor_rects_.end()) {
    return;
  }
  const CefRect& rect = cursor->second;
  SendMessage(kImeCursorAreaChanged, std::max(1, rect.width),
              std::max(1, rect.height), rect.x + offset_x, rect.y + offset_y,
              nullptr, 0);
}
