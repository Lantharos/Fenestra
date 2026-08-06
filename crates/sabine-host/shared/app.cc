#include "app.h"

#include <sstream>
#include <string>
#include <vector>

#include "sabine_bridge_js.h"
#include "include/cef_browser.h"
#include "include/cef_command_line.h"
#include "include/wrapper/cef_helpers.h"
#include "osr_handler.h"

namespace {
std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line) {
  std::vector<std::string> commands;
  std::stringstream stream(
      std::string(command_line->GetSwitchValue("sabine-bridge-commands")));
  std::string item;
  while (std::getline(stream, item, ',')) {
    if (!item.empty()) {
      commands.push_back(item);
    }
  }
  return commands;
}

std::string JsString(const std::string& value) {
  std::string output = "\"";
  for (char character : value) {
    switch (character) {
      case '\\': output += "\\\\"; break;
      case '"': output += "\\\""; break;
      case '\n': output += "\\n"; break;
      case '\r': output += "\\r"; break;
      case '\t': output += "\\t"; break;
      default: output += character; break;
    }
  }
  return output + "\"";
}

std::string BridgeInstallScript(const std::vector<std::string>& commands) {
  std::string list = "[";
  for (size_t index = 0; index < commands.size(); ++index) {
    if (index > 0) {
      list += ",";
    }
    list += JsString(commands[index]);
  }
  return "window.__sabineBridgeCommands=" + list + "];" +
         SABINE_BRIDGE_JS_RAW;
}

void CreateBrowser(CefRefPtr<CefCommandLine> command_line) {
  CreateSabineOsrBrowser(command_line);
}
}  // namespace

SabineApp::SabineApp() = default;

void SabineApp::OnBeforeCommandLineProcessing(
    const CefString& process_type,
    CefRefPtr<CefCommandLine> command_line) {
  const bool software_osr = command_line->HasSwitch("sabine-software-osr");
  const std::string ozone_platform =
      command_line->GetSwitchValue("sabine-ozone-platform");
  // Chromium rejects Wayland ozone + Vulkan. Prefer X11 ozone for OSR; if a
  // caller still forces Wayland, disable Vulkan so GPU init can proceed.
  const bool disable_vulkan =
      software_osr || ozone_platform == "wayland";
  if (disable_vulkan) {
    command_line->AppendSwitch("disable-vulkan");
  }
  if (!ozone_platform.empty()) {
    command_line->AppendSwitchWithValue("ozone-platform", ozone_platform);
    command_line->AppendSwitchWithValue("ozone-platform-hint", ozone_platform);
  }

  // Merge into any disable-features already set on the argv (do not replace).
  std::string disabled =
      command_line->GetSwitchValue("disable-features").ToString();
  auto ends_with_comma_or_empty = [&disabled]() {
    return disabled.empty() || disabled.back() == ',';
  };
  auto append_csv = [&disabled, &ends_with_comma_or_empty](const char* csv) {
    if (!ends_with_comma_or_empty()) {
      disabled += ",";
    }
    disabled += csv;
  };
  if (disabled.find("OptimizationGuideOnDeviceModel") == std::string::npos) {
    append_csv("OptimizationGuideOnDeviceModel");
  }
  if (disable_vulkan) {
    append_csv("Vulkan,DefaultANGLEVulkan,VulkanFromANGLE");
  }
  command_line->AppendSwitchWithValue("disable-features", disabled);

  command_line->AppendSwitchWithValue("password-store", "basic");
  if (command_line->HasSwitch("sabine-transparent")) {
    command_line->AppendSwitch("enable-transparent-visuals");
    command_line->AppendSwitch("transparent-painting-enabled");
    command_line->AppendSwitchWithValue("default-background-color", "0x00000000");
  }
}

void SabineApp::OnContextInitialized() {
  CEF_REQUIRE_UI_THREAD();
  CreateBrowser(CefCommandLine::GetGlobalCommandLine());
}

void SabineApp::OnBrowserCreated(CefRefPtr<CefBrowser> browser,
                                  CefRefPtr<CefDictionaryValue> extra_info) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (browser && extra_info && extra_info->HasKey("sabineAllowBridge") &&
      !extra_info->GetBool("sabineAllowBridge")) {
    unprivileged_browsers_.insert(browser->GetIdentifier());
  }
}

void SabineApp::OnBrowserDestroyed(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (browser) {
    unprivileged_browsers_.erase(browser->GetIdentifier());
  }
}

void SabineApp::OnContextCreated(CefRefPtr<CefBrowser> browser,
                                  CefRefPtr<CefFrame> frame,
                                  CefRefPtr<CefV8Context> context) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (!frame->IsMain() ||
      (browser && unprivileged_browsers_.find(browser->GetIdentifier()) !=
                      unprivileged_browsers_.end())) {
    return;
  }
  const auto commands = BridgeCommands(CefCommandLine::GetGlobalCommandLine());
  if (!commands.empty()) {
    frame->ExecuteJavaScript(BridgeInstallScript(commands), frame->GetURL(), 0);
  }
}

bool SabineApp::OnAlreadyRunningAppRelaunch(
    CefRefPtr<CefCommandLine> command_line,
    const CefString& current_directory) {
  CEF_REQUIRE_UI_THREAD();
  CreateBrowser(command_line);
  return true;
}

CefRefPtr<CefClient> SabineApp::GetDefaultClient() {
  return SabineOsrHandler::GetInstance();
}
