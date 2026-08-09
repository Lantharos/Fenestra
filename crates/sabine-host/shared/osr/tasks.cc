#include "osr/tasks.h"

#include <utility>

namespace sabine_osr {

OsrCommandTask::OsrCommandTask(CefRefPtr<SabineOsrHandler> handler,
                               std::string line)
    : handler_(std::move(handler)), line_(std::move(line)) {}

OsrCommandTask::~OsrCommandTask() = default;

void OsrCommandTask::Execute() {
  handler_->HandleControlLine(line_);
}

OsrResizeTask::OsrResizeTask(CefRefPtr<SabineOsrHandler> handler)
    : handler_(std::move(handler)) {}

OsrResizeTask::~OsrResizeTask() = default;

void OsrResizeTask::Execute() {
  handler_->HandlePendingResize();
}

CloseOnDisconnectTask::CloseOnDisconnectTask(
    CefRefPtr<SabineOsrHandler> handler)
    : handler_(std::move(handler)) {}

CloseOnDisconnectTask::~CloseOnDisconnectTask() = default;

void CloseOnDisconnectTask::Execute() {
  handler_->CloseFromNativeDisconnect();
}

}  // namespace sabine_osr
