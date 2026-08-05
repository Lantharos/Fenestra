#include "app.h"

#include <sstream>
#include <string>
#include <vector>

#include "mullion_bridge_js.h"
#include "include/cef_browser.h"
#include "include/cef_command_line.h"
#include "include/wrapper/cef_helpers.h"
#include "osr_handler.h"

namespace {
std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line) {
  std::vector<std::string> commands;
  std::stringstream stream(
      std::string(command_line->GetSwitchValue("mullion-bridge-commands")));
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
  return "window.__mullionBridgeCommands=" + list + "];" +
         MULLION_BRIDGE_JS_RAW;
}

void CreateBrowser(CefRefPtr<CefCommandLine> command_line) {
  CreateMullionOsrBrowser(command_line);
}
}  // namespace

MullionApp::MullionApp() = default;

void MullionApp::OnBeforeCommandLineProcessing(
    const CefString& process_type,
    CefRefPtr<CefCommandLine> command_line) {
  command_line->AppendSwitch("disable-vulkan");
  const std::string ozone_platform =
      command_line->GetSwitchValue("mullion-ozone-platform");
  if (!ozone_platform.empty()) {
    command_line->AppendSwitchWithValue("ozone-platform", ozone_platform);
    command_line->AppendSwitchWithValue("ozone-platform-hint", ozone_platform);
  }
  command_line->AppendSwitchWithValue(
      "disable-features",
      "Vulkan,DefaultANGLEVulkan,VulkanFromANGLE,OptimizationGuideOnDeviceModel");
  command_line->AppendSwitchWithValue("password-store", "basic");
  if (command_line->HasSwitch("mullion-transparent")) {
    command_line->AppendSwitch("enable-transparent-visuals");
    command_line->AppendSwitch("transparent-painting-enabled");
    command_line->AppendSwitchWithValue("default-background-color", "0x00000000");
  }
}

void MullionApp::OnContextInitialized() {
  CEF_REQUIRE_UI_THREAD();
  CreateBrowser(CefCommandLine::GetGlobalCommandLine());
}

void MullionApp::OnBrowserCreated(CefRefPtr<CefBrowser> browser,
                                  CefRefPtr<CefDictionaryValue> extra_info) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (browser && extra_info && extra_info->HasKey("mullionAllowBridge") &&
      !extra_info->GetBool("mullionAllowBridge")) {
    unprivileged_browsers_.insert(browser->GetIdentifier());
  }
}

void MullionApp::OnBrowserDestroyed(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (browser) {
    unprivileged_browsers_.erase(browser->GetIdentifier());
  }
}

void MullionApp::OnContextCreated(CefRefPtr<CefBrowser> browser,
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

bool MullionApp::OnAlreadyRunningAppRelaunch(
    CefRefPtr<CefCommandLine> command_line,
    const CefString& current_directory) {
  CEF_REQUIRE_UI_THREAD();
  CreateBrowser(command_line);
  return true;
}

CefRefPtr<CefClient> MullionApp::GetDefaultClient() {
  return MullionOsrHandler::GetInstance();
}
