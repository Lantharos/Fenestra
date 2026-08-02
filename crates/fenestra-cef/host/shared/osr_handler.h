#ifndef FENESTRA_CEF_HOST_OSR_HANDLER_H_
#define FENESTRA_CEF_HOST_OSR_HANDLER_H_

#include <cstdint>
#include <list>
#include <map>
#include <mutex>
#include <set>
#include <string>
#include <vector>

#include "guest_manager.h"
#include "include/cef_client.h"
#include "include/cef_command_line.h"
#include "include/cef_context_menu_handler.h"
#include "include/cef_display_handler.h"
#include "include/cef_download_handler.h"
#include "include/cef_render_handler.h"
#include "include/cef_request_context.h"
#include "include/cef_values.h"

constexpr uint32_t kMainFrame = 1;
constexpr uint32_t kPopupFrame = 2;
constexpr uint32_t kPopupHidden = 3;
constexpr uint32_t kMainBatch = 12;
constexpr uint32_t kPopupBatch = 13;
constexpr uint32_t kMainSharedBatch = 14;
constexpr uint32_t kPopupSharedBatch = 15;
constexpr uint32_t kFileDragRequested = 16;
constexpr uint32_t kGuestFrame = 17;
constexpr uint32_t kGuestBatch = 18;
constexpr uint32_t kGuestSharedBatch = 19;
constexpr uint32_t kGuestHidden = 20;
constexpr uint32_t kDraggableRegionsChanged = 21;

class FenestraOsrHandler : public CefClient,
                       public CefContextMenuHandler,
                       public CefDisplayHandler,
                       public CefDownloadHandler,
                       public CefDragHandler,
                       public CefLifeSpanHandler,
                       public CefLoadHandler,
                       public CefRenderHandler,
                       public CefRequestHandler {
 public:
  FenestraOsrHandler(std::string socket_path,
                 int width,
                 int height,
	                 float scale,
	                 std::vector<std::string> bridge_commands,
	                 bool transparent_background,
	                 int active_frame_rate,
	                 int background_frame_rate);
  ~FenestraOsrHandler() override;

  static FenestraOsrHandler* GetInstance();

  CefRefPtr<CefContextMenuHandler> GetContextMenuHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }
  CefRefPtr<CefDownloadHandler> GetDownloadHandler() override { return this; }
  CefRefPtr<CefDragHandler> GetDragHandler() override { return this; }
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefRenderHandler> GetRenderHandler() override { return this; }
  CefRefPtr<CefRequestHandler> GetRequestHandler() override { return this; }

  void OnBeforeContextMenu(CefRefPtr<CefBrowser> browser,
                           CefRefPtr<CefFrame> frame,
                           CefRefPtr<CefContextMenuParams> params,
                           CefRefPtr<CefMenuModel> model) override;
  bool OnContextMenuCommand(CefRefPtr<CefBrowser> browser,
                            CefRefPtr<CefFrame> frame,
                            CefRefPtr<CefContextMenuParams> params,
                            int command_id,
                            EventFlags event_flags) override;
  bool OnCursorChange(CefRefPtr<CefBrowser> browser,
                      CefCursorHandle cursor,
                      cef_cursor_type_t type,
                      const CefCursorInfo& custom_cursor_info) override;
  void OnTitleChange(CefRefPtr<CefBrowser> browser,
                     const CefString& title) override;
  void OnAddressChange(CefRefPtr<CefBrowser> browser,
                       CefRefPtr<CefFrame> frame,
                       const CefString& url) override;
  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override;
  bool OnBeforePopup(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefFrame> frame,
                     FENESTRA_CEF_POPUP_ID
                     const CefString& target_url,
                     const CefString& target_frame_name,
                     cef_window_open_disposition_t target_disposition,
                     bool user_gesture,
                     const CefPopupFeatures& popup_features,
                     CefWindowInfo& window_info,
                     CefRefPtr<CefClient>& client,
                     CefBrowserSettings& settings,
                     CefRefPtr<CefDictionaryValue>& extra_info,
                     bool* no_javascript_access) override;
  bool DoClose(CefRefPtr<CefBrowser> browser) override;
  void OnBeforeClose(CefRefPtr<CefBrowser> browser) override;
  void OnLoadError(CefRefPtr<CefBrowser> browser,
                   CefRefPtr<CefFrame> frame,
                   ErrorCode errorCode,
                   const CefString& errorText,
                   const CefString& failedUrl) override;
  void OnLoadStart(CefRefPtr<CefBrowser> browser,
                   CefRefPtr<CefFrame> frame,
                   TransitionType transition_type) override;
  void OnLoadEnd(CefRefPtr<CefBrowser> browser,
                 CefRefPtr<CefFrame> frame,
                 int httpStatusCode) override;
  void OnLoadingStateChange(CefRefPtr<CefBrowser> browser,
                            bool isLoading,
                            bool canGoBack,
                            bool canGoForward) override;
  bool OnBeforeBrowse(CefRefPtr<CefBrowser> browser,
                      CefRefPtr<CefFrame> frame,
                      CefRefPtr<CefRequest> request,
                      bool user_gesture,
                      bool is_redirect) override;

  bool OnBeforeDownload(CefRefPtr<CefBrowser> browser,
                        CefRefPtr<CefDownloadItem> download_item,
                        const CefString& suggested_name,
                        CefRefPtr<CefBeforeDownloadCallback> callback) override;
  void OnDownloadUpdated(CefRefPtr<CefBrowser> browser,
                         CefRefPtr<CefDownloadItem> download_item,
                         CefRefPtr<CefDownloadItemCallback> callback) override;
  void OnDraggableRegionsChanged(
      CefRefPtr<CefBrowser> browser,
      CefRefPtr<CefFrame> frame,
      const std::vector<CefDraggableRegion>& regions) override;

  bool GetScreenInfo(CefRefPtr<CefBrowser> browser,
                     CefScreenInfo& screen_info) override;
  void GetViewRect(CefRefPtr<CefBrowser> browser, CefRect& rect) override;
  void OnPopupShow(CefRefPtr<CefBrowser> browser, bool show) override;
  void OnPopupSize(CefRefPtr<CefBrowser> browser, const CefRect& rect) override;
  void OnPaint(CefRefPtr<CefBrowser> browser,
               PaintElementType type,
               const RectList& dirtyRects,
               const void* buffer,
               int width,
               int height) override;
  bool StartDragging(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefDragData> drag_data,
                     cef_drag_operations_mask_t allowed_ops,
                     int x,
                     int y) override;
  void UpdateDragCursor(CefRefPtr<CefBrowser> browser,
                        DragOperation operation) override;

  void HandleControlLine(const std::string& line);
  void ApplyHostControl(const std::string& command, const std::string& value);
  void ResolveBridgeResponse(const std::string& browser_id,
                             const std::string& request_id,
                             bool ok,
                             const std::string& payload);
  void EmitBridgeEvent(const std::string& name_json,
                       const std::string& payload);

 private:
  using BrowserList = std::list<CefRefPtr<CefBrowser>>;

  bool ConnectSocket();
  bool SendMessage(uint32_t kind,
                   uint32_t width,
                   uint32_t height,
                   int32_t x,
                   int32_t y,
                   const void* payload,
                   uint32_t payload_len);
  bool SendMessageWithFd(uint32_t kind,
                         uint32_t width,
                         uint32_t height,
                         int32_t x,
                         int32_t y,
                         const void* payload,
                         uint32_t payload_len,
                         int fd);
  bool SendPaintBatch(uint32_t kind,
                      const std::string& guest_id,
                      int32_t origin_x,
                      int32_t origin_y,
                      const void* buffer,
                      int buffer_width,
                      int buffer_height,
                      const RectList& dirty_rects);
  bool HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
                           CefRefPtr<CefFrame> frame,
                           const std::string& url);
  bool HandleWindowCommand(CefRefPtr<CefBrowser> browser, const std::string& url);
  void RequestNativeClose();
	  void InstallBridge(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame);
	  void InstallTransparentBackground(CefRefPtr<CefFrame> frame);
	  void ApplyLifecycle(const std::string& state, int frame_rate, const std::string& reason);
	  void DispatchLifecycle(const std::string& state, const std::string& reason);
	  void StartCommandReader();

  GuestView* GuestForBrowser(const CefRefPtr<CefBrowser>& browser);
  bool HandleGuestBridgeCommand(const std::string& command,
                                const std::string& payload,
                                const std::string& browser_id,
                                const std::string& request_id);
  bool RunGuestOperation(const std::string& operation,
                         const std::string& payload,
                         std::string* response,
                         std::string* error);
  bool CreateGuest(const GuestCreateRequest& request, std::string* error);
  void DestroyGuest(const std::string& id);
  void FocusGuest(const std::string& id);
  void DismissPopupGuest();
  void ApplyGuestBounds(GuestView& guest);
  void ApplyGuestVisibility(GuestView& guest);
  void ApplyGuestLifecycle();
  void NotifyGuestScreenInfo();
  bool SendGuestPaint(const GuestView& guest,
                      const void* buffer,
                      int width,
                      int height,
                      const RectList& dirty_rects);
  void SendGuestHidden(const GuestView& guest);
  void EmitPrimaryEvent(const std::string& name, const std::string& payload);
  bool RunGuestDownloadAction(const std::string& payload, std::string* error);
  CefRefPtr<CefRequestContext> GuestRequestContext(const std::string& partition);
  std::string NextGuestId();

  BrowserList browsers_;
  CefRefPtr<CefBrowser> browser_;
  std::string socket_path_;
  int socket_fd_ = -1;
  std::mutex socket_mutex_;
  int width_ = 1;
  int height_ = 1;
  float scale_ = 1.0f;
  CefRect popup_rect_;
  GuestRegistry guests_;
  std::string focused_guest_id_;
  std::string pending_guest_id_;
  std::map<std::string, CefRefPtr<CefRequestContext>> guest_contexts_;
  std::map<std::string, GuestDownload> downloads_;
  int guest_serial_ = 0;
	  std::set<std::string> bridge_commands_;
	  bool transparent_background_ = false;
	  bool suspended_ = false;
	  int active_frame_rate_ = 60;
	  int background_frame_rate_ = 5;
	  bool closing_ = false;

  IMPLEMENT_REFCOUNTING(FenestraOsrHandler);
};

void CreateFenestraOsrBrowser(CefRefPtr<CefCommandLine> command_line);

#endif
