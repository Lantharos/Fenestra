#ifndef SABINE_CEF_HOST_OSR_TASKS_H_
#define SABINE_CEF_HOST_OSR_TASKS_H_

#include <string>

#include "include/cef_task.h"
#include "osr/handler.h"

namespace sabine_osr {

class OsrCommandTask : public CefTask {
 public:
  OsrCommandTask(CefRefPtr<SabineOsrHandler> handler, std::string line);
  ~OsrCommandTask() override;
  void Execute() override;

 private:
  CefRefPtr<SabineOsrHandler> handler_;
  const std::string line_;
  IMPLEMENT_REFCOUNTING(OsrCommandTask);
};

class OsrResizeTask : public CefTask {
 public:
  explicit OsrResizeTask(CefRefPtr<SabineOsrHandler> handler);
  ~OsrResizeTask() override;
  void Execute() override;

 private:
  CefRefPtr<SabineOsrHandler> handler_;
  IMPLEMENT_REFCOUNTING(OsrResizeTask);
};

class CloseOnDisconnectTask : public CefTask {
 public:
  explicit CloseOnDisconnectTask(CefRefPtr<SabineOsrHandler> handler);
  ~CloseOnDisconnectTask() override;
  void Execute() override;

 private:
  CefRefPtr<SabineOsrHandler> handler_;
  IMPLEMENT_REFCOUNTING(CloseOnDisconnectTask);
};

}  // namespace sabine_osr

#endif
