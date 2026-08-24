#include "app/app.h"

#if defined(OS_WIN) || defined(_WIN32)
#include <windows.h>
#endif

#if defined(CEF_X11)
#include <X11/Xlib.h>
#endif

#include <cstdlib>
#include <string>

#include "include/base/cef_compiler_specific.h"
#include "include/cef_app.h"
#include "include/cef_command_line.h"
#include "entry.h"

#if defined(CEF_X11)
namespace {
int XErrorHandlerImpl(Display* display, XErrorEvent* event) {
  return 0;
}

int XIOErrorHandlerImpl(Display* display) {
  return 0;
}
}  // namespace
#endif

#if defined(OS_LINUX)
NO_STACK_PROTECTOR
#endif
int RunSabineHost(CefMainArgs main_args, int argc, char* argv[]) {
  CefRefPtr<CefCommandLine> command_line = CefCommandLine::CreateCommandLine();
#if defined(OS_WIN) || defined(_WIN32)
  command_line->InitFromString(::GetCommandLineW());
#else
  command_line->InitFromArgv(argc, argv);
#endif
  const bool runtime_smoke_test =
      command_line->HasSwitch("sabine-runtime-smoke-test");
  CefRefPtr<SabineApp> app(new SabineApp(runtime_smoke_test));

  int exit_code = CefExecuteProcess(main_args, app.get(), nullptr);
  if (exit_code >= 0) {
    return exit_code;
  }

#if defined(CEF_X11)
  XSetErrorHandler(XErrorHandlerImpl);
  XSetIOErrorHandler(XIOErrorHandlerImpl);
#endif

  CefSettings settings;
  settings.no_sandbox = true;
  settings.windowless_rendering_enabled = true;

  const std::string framework_dir_path =
      command_line->GetSwitchValue("sabine-framework-dir-path");
  if (!framework_dir_path.empty()) {
    CefString(&settings.framework_dir_path).FromString(framework_dir_path);
  }

  const std::string resources_dir_path =
      command_line->GetSwitchValue("sabine-resources-dir-path");
  if (!resources_dir_path.empty()) {
    CefString(&settings.resources_dir_path).FromString(resources_dir_path);
  }

  const std::string locales_dir_path =
      command_line->GetSwitchValue("sabine-locales-dir-path");
  if (!locales_dir_path.empty()) {
    CefString(&settings.locales_dir_path).FromString(locales_dir_path);
  }

  const std::string root_cache_path =
      command_line->GetSwitchValue("root-cache-path");
  if (!root_cache_path.empty()) {
    CefString(&settings.root_cache_path).FromString(root_cache_path);
  }

  const std::string cache_path = command_line->GetSwitchValue("cache-path");
  if (!cache_path.empty()) {
    CefString(&settings.cache_path).FromString(cache_path);
  }

  if (!CefInitialize(main_args, settings, app.get(), nullptr)) {
    return CefGetExitCode();
  }

  if (runtime_smoke_test) {
    std::_Exit(0);
  }

  CefRunMessageLoop();
  CefShutdown();
  return 0;
}

#if defined(OS_WIN) || defined(_WIN32)
int APIENTRY wWinMain(HINSTANCE hInstance,
                      HINSTANCE hPrevInstance,
                      LPWSTR lpCmdLine,
                      int nCmdShow) {
  (void)hPrevInstance;
  (void)lpCmdLine;
  (void)nCmdShow;
  CefMainArgs main_args(hInstance);
  return RunSabineHost(main_args, __argc, __argv);
}
#elif !defined(OS_MAC)
NO_STACK_PROTECTOR
int main(int argc, char* argv[]) {
  CefMainArgs main_args(argc, argv);
  return RunSabineHost(main_args, argc, argv);
}
#endif
