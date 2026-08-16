#import <Cocoa/Cocoa.h>

#include <string>

#include "entry.h"
#include "include/cef_application_mac.h"
#include "include/wrapper/cef_helpers.h"
#include "include/wrapper/cef_library_loader.h"

@interface SabineApplication : NSApplication <CefAppProtocol> {
 @private
  BOOL handlingSendEvent_;
}
@end

@implementation SabineApplication
- (BOOL)isHandlingSendEvent {
  return handlingSendEvent_;
}

- (void)setHandlingSendEvent:(BOOL)handlingSendEvent {
  handlingSendEvent_ = handlingSendEvent;
}

- (void)sendEvent:(NSEvent*)event {
  CefScopedSendingEvent sendingEvent;
  [super sendEvent:event];
}
@end

int main(int argc, char* argv[]) {
  bool subprocess = false;
  for (int index = 1; index < argc; ++index) {
    const std::string argument(argv[index]);
    if (argument.rfind("--type=", 0) == 0 || argument == "--type") {
      subprocess = true;
      break;
    }
  }

  CefScopedLibraryLoader library_loader;
  if (subprocess ? !library_loader.LoadInHelper()
                 : !library_loader.LoadInMain()) {
    return 1;
  }

  @autoreleasepool {
    if (!subprocess) {
      [SabineApplication sharedApplication];
      CHECK([NSApp isKindOfClass:[SabineApplication class]]);
    }
    CefMainArgs main_args(argc, argv);
    return RunSabineHost(main_args, argc, argv);
  }
}
