#ifndef MULLION_CEF_HOST_GUEST_INPUT_H_
#define MULLION_CEF_HOST_GUEST_INPUT_H_

#include <cstdint>
#include <string>
#include <vector>

constexpr uint32_t kGuestModShift = 1u << 1;
constexpr uint32_t kGuestModControl = 1u << 2;
constexpr uint32_t kGuestModAlt = 1u << 3;
constexpr uint32_t kGuestModCommand = 1u << 7;
constexpr uint32_t kGuestModRepeat = 1u << 13;
constexpr uint32_t kGuestModMask =
    kGuestModShift | kGuestModControl | kGuestModAlt | kGuestModCommand;

std::string GuestShortcutJson(const std::string& id,
                              const std::string& accelerator,
                              const std::string& key,
                              bool repeat,
                              uint32_t modifiers);
std::string GuestWheelJson(const std::string& id,
                           double delta_x,
                           double delta_y,
                           uint32_t modifiers);
std::string GuestFaviconJson(const std::string& id,
                             const std::vector<std::string>& favicons);

const std::string* MatchInterceptedShortcut(
    const std::vector<std::string>& shortcuts,
    const std::string& key,
    uint32_t modifiers);

bool IsPredominantlyHorizontalWheel(double delta_x, double delta_y);

#endif
