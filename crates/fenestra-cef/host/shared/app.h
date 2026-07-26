#ifndef FENESTRA_CEF_HOST_APP_H_
#define FENESTRA_CEF_HOST_APP_H_

#include <set>

#include "include/cef_app.h"
#include "include/cef_render_process_handler.h"
#include "include/cef_v8.h"

class FenestraApp : public CefApp,
                public CefBrowserProcessHandler,
                public CefRenderProcessHandler {
 public:
  FenestraApp();

  CefRefPtr<CefBrowserProcessHandler> GetBrowserProcessHandler() override {
    return this;
  }
  CefRefPtr<CefRenderProcessHandler> GetRenderProcessHandler() override {
    return this;
  }

  void OnBeforeCommandLineProcessing(
      const CefString& process_type,
      CefRefPtr<CefCommandLine> command_line) override;
  void OnContextInitialized() override;
  void OnBrowserCreated(CefRefPtr<CefBrowser> browser,
                        CefRefPtr<CefDictionaryValue> extra_info) override;
  void OnBrowserDestroyed(CefRefPtr<CefBrowser> browser) override;
  void OnContextCreated(CefRefPtr<CefBrowser> browser,
                        CefRefPtr<CefFrame> frame,
                        CefRefPtr<CefV8Context> context) override;
  bool OnAlreadyRunningAppRelaunch(
      CefRefPtr<CefCommandLine> command_line,
      const CefString& current_directory) override;
  CefRefPtr<CefClient> GetDefaultClient() override;

 private:
  std::set<int> unprivileged_browsers_;

  IMPLEMENT_REFCOUNTING(FenestraApp);
};

#endif
