#include "osr/handler.h"

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <limits>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <sys/socket.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <unistd.h>
#endif

#include "guest/input.h"
#include "guest/manager.h"
#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_parser.h"
#include "include/cef_request_context_handler.h"
#include "include/cef_task.h"
#include "include/internal/cef_types.h"
#include "include/wrapper/cef_helpers.h"
#include "common/json.h"
#include "sabine_bridge_js.h"
#include "osr/accelerated/paint.h"
#include "osr/utilities.h"

using namespace sabine_osr;

SabineOsrHandler::SabineOsrHandler(std::string endpoint,
                               std::string authentication_token,
                               int width,
	                               int height,
	                               float scale,
	                               std::vector<std::string> bridge_commands,
	                               bool transparent_background,
	                               int active_frame_rate,
	                               int background_frame_rate)
	    : endpoint_(std::move(endpoint)),
	      authentication_token_(std::move(authentication_token)),
	      width_(std::max(1, width)),
	      height_(std::max(1, height)),
	      scale_(std::max(0.25f, scale)),
	      bridge_commands_(bridge_commands.begin(), bridge_commands.end()),
	      transparent_background_(transparent_background),
	      active_frame_rate_(std::max(1, active_frame_rate)),
	      background_frame_rate_(std::max(1, background_frame_rate)) {
  if (!g_instance) {
    g_instance = this;
  }
  RegisterHandler(this);
}

SabineOsrHandler::~SabineOsrHandler() {
  if (socket_fd_ >= 0) {
#ifdef _WIN32
    closesocket(static_cast<SOCKET>(socket_fd_));
    WSACleanup();
#else
    close(socket_fd_);
#endif
  }
  UnregisterHandler(this);
  if (g_instance == this) {
    g_instance = nullptr;
    const auto remaining = SnapshotHandlers();
    if (!remaining.empty()) {
      g_instance = remaining.front();
    }
  }
}

SabineOsrHandler* SabineOsrHandler::GetInstance() {
  return g_instance;
}

void CreateSabineOsrBrowser(CefRefPtr<CefCommandLine> command_line) {
  const std::string url_value = command_line->GetSwitchValue("url");
  const std::string url =
      url_value.empty() ? "about:blank" : std::string(url_value);
	  const int width = std::max(1, SwitchInt(command_line, "sabine-width", 800));
	  const int height = std::max(1, SwitchInt(command_line, "sabine-height", 600));
	  const float scale = SwitchFloat(command_line, "sabine-scale", 1.0f);
	  const int active_frame_rate =
	      std::max(1, SwitchInt(command_line, "sabine-active-frame-rate", 60));
	  const int background_frame_rate =
	      std::max(1, SwitchInt(command_line, "sabine-background-frame-rate", 5));
	  const std::string endpoint = command_line->GetSwitchValue("sabine-osr-endpoint");
	  std::string authentication_token;
	  const std::string token_file =
	      command_line->GetSwitchValue("sabine-osr-token-file");
	  if (!token_file.empty()) {
	    std::ifstream input(token_file.c_str(), std::ios::in | std::ios::binary);
	    if (input) {
	      std::getline(input, authentication_token);
	      // Strip trailing CR from Windows files.
	      while (!authentication_token.empty() &&
	             (authentication_token.back() == '\r' ||
	              authentication_token.back() == '\n')) {
	        authentication_token.pop_back();
	      }
	    }
	    // Remove after read so the secret does not linger. Handoff still works
	    // because the secondary process writes a fresh file and the primary
	    // reads it from the relaunch command line before this unlink.
	    std::remove(token_file.c_str());
	  }
	  if (authentication_token.empty()) {
	    if (const char* token_env = std::getenv("SABINE_OSR_TOKEN")) {
	      authentication_token = token_env;
#if defined(_WIN32)
	      _putenv_s("SABINE_OSR_TOKEN", "");
#else
	      unsetenv("SABINE_OSR_TOKEN");
#endif
	    }
	  }
	  if (authentication_token.empty()) {
	    std::cerr << "Sabine OSR: missing authentication token "
	                 "(expected --sabine-osr-token-file or SABINE_OSR_TOKEN)"
	              << std::endl;
	  }

	  CefBrowserSettings browser_settings;
	  browser_settings.windowless_frame_rate = active_frame_rate;
  if (command_line->HasSwitch("sabine-transparent")) {
    browser_settings.background_color = CefColorSetARGB(0, 0, 0, 0);
  }

	  CefWindowInfo window_info;
	  window_info.SetAsWindowless(kNullWindowHandle);
	  sabine_osr::ApplySharedTexture(
	      &window_info, sabine_osr::PreferSharedTexture(command_line));
	  CefRefPtr<SabineOsrHandler> handler(new SabineOsrHandler(
	      endpoint, authentication_token, width, height, scale,
	      BridgeCommands(command_line),
	      command_line->HasSwitch("sabine-transparent"), active_frame_rate,
	      background_frame_rate));
  CefBrowserHost::CreateBrowser(window_info, handler, url, browser_settings,
                                nullptr, nullptr);
}
