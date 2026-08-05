#include "guest_input.h"

#include <cctype>
#include <cmath>
#include <sstream>
#include <vector>

#include "json_util.h"

namespace {

std::string BoolLiteral(bool value) {
  return value ? "true" : "false";
}

std::string ToLowerAscii(std::string value) {
  for (char& c : value) {
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  }
  return value;
}

std::string NormalizeShortcutKey(std::string key) {
  key = ToLowerAscii(std::move(key));
  if (key.rfind("key", 0) == 0 && key.size() == 4) {
    return key.substr(3);
  }
  if (key == " " || key == "space") {
    return "space";
  }
  return key;
}

uint32_t PlatformPrimaryModifier() {
#if defined(OS_MAC) || defined(__APPLE__)
  return kGuestModCommand;
#else
  return kGuestModControl;
#endif
}

bool ParseAccelerator(const std::string& accelerator,
                      uint32_t* required_mods,
                      std::string* required_key) {
  *required_mods = 0;
  required_key->clear();
  if (accelerator.empty()) {
    return false;
  }
  std::string token;
  std::vector<std::string> parts;
  for (size_t i = 0; i <= accelerator.size(); ++i) {
    if (i == accelerator.size() || accelerator[i] == '+') {
      if (!token.empty()) {
        parts.push_back(token);
        token.clear();
      }
      continue;
    }
    token += accelerator[i];
  }
  if (parts.empty()) {
    return false;
  }
  *required_key = NormalizeShortcutKey(parts.back());
  parts.pop_back();
  if (required_key->empty()) {
    return false;
  }
  for (const std::string& part : parts) {
    const std::string mod = ToLowerAscii(part);
    if (mod == "primary") {
      *required_mods |= PlatformPrimaryModifier();
    } else if (mod == "control" || mod == "ctrl") {
      *required_mods |= kGuestModControl;
    } else if (mod == "meta" || mod == "command" || mod == "cmd") {
      *required_mods |= kGuestModCommand;
    } else if (mod == "alt" || mod == "option") {
      *required_mods |= kGuestModAlt;
    } else if (mod == "shift") {
      *required_mods |= kGuestModShift;
    } else {
      return false;
    }
  }
  return true;
}

}  // namespace

std::string GuestShortcutJson(const std::string& id,
                              const std::string& accelerator,
                              const std::string& key,
                              bool repeat,
                              uint32_t modifiers) {
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(id) << "\",\"accelerator\":\""
         << JsonEscape(accelerator) << "\",\"key\":\"" << JsonEscape(key)
         << "\",\"repeat\":" << BoolLiteral(repeat) << ",\"ctrlKey\":"
         << BoolLiteral((modifiers & kGuestModControl) != 0) << ",\"metaKey\":"
         << BoolLiteral((modifiers & kGuestModCommand) != 0) << ",\"altKey\":"
         << BoolLiteral((modifiers & kGuestModAlt) != 0) << ",\"shiftKey\":"
         << BoolLiteral((modifiers & kGuestModShift) != 0) << "}";
  return output.str();
}

std::string GuestWheelJson(const std::string& id,
                           double delta_x,
                           double delta_y,
                           uint32_t modifiers) {
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(id) << "\",\"deltaX\":" << delta_x
         << ",\"deltaY\":" << delta_y << ",\"ctrlKey\":"
         << BoolLiteral((modifiers & kGuestModControl) != 0) << ",\"metaKey\":"
         << BoolLiteral((modifiers & kGuestModCommand) != 0) << ",\"altKey\":"
         << BoolLiteral((modifiers & kGuestModAlt) != 0) << ",\"shiftKey\":"
         << BoolLiteral((modifiers & kGuestModShift) != 0) << "}";
  return output.str();
}

std::string GuestFaviconJson(const std::string& id,
                             const std::vector<std::string>& favicons) {
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(id) << "\",\"favicons\":[";
  for (size_t i = 0; i < favicons.size(); ++i) {
    if (i > 0) {
      output << ',';
    }
    output << '"' << JsonEscape(favicons[i]) << '"';
  }
  output << "]}";
  return output.str();
}

const std::string* MatchInterceptedShortcut(
    const std::vector<std::string>& shortcuts,
    const std::string& key,
    uint32_t modifiers) {
  const std::string normalized_key = NormalizeShortcutKey(key);
  const uint32_t pressed = modifiers & kGuestModMask;
  for (const std::string& accelerator : shortcuts) {
    uint32_t required_mods = 0;
    std::string required_key;
    if (!ParseAccelerator(accelerator, &required_mods, &required_key)) {
      continue;
    }
    if (required_key == normalized_key && required_mods == pressed) {
      return &accelerator;
    }
  }
  return nullptr;
}

bool IsPredominantlyHorizontalWheel(double delta_x, double delta_y) {
  const double abs_x = std::fabs(delta_x);
  const double abs_y = std::fabs(delta_y);
  return abs_x > 0.75 && abs_x >= abs_y * 0.9;
}

