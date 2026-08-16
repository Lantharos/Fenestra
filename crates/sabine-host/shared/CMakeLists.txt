cmake_minimum_required(VERSION 3.21)

project(sabine_cef_host)

set(CMAKE_CONFIGURATION_TYPES Debug Release)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(SABINE_CEF_API_VERSION "15101" CACHE STRING "Stable CEF API used by Sabine")
add_compile_definitions(CEF_API_VERSION=${SABINE_CEF_API_VERSION})

if(NOT CEF_ROOT)
  message(FATAL_ERROR "CEF_ROOT is required")
endif()

# CEF macros interpolate CEF_LIB_* into CMake strings; backslashes become escapes.
file(TO_CMAKE_PATH "${CEF_ROOT}" CEF_ROOT)

if(WIN32 AND CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
  message(FATAL_ERROR
    "sabine-host on Windows requires MSVC. Official CEF binaries cannot be linked with MinGW/MSYS.")
endif()

set(CMAKE_MODULE_PATH ${CMAKE_MODULE_PATH} "${CEF_ROOT}/cmake")
find_package(CEF REQUIRED)

if(OS_MAC)
  enable_language(OBJCXX)
  set(CMAKE_OBJCXX_STANDARD 20)
  set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)
endif()

add_subdirectory(${CEF_LIBCEF_DLL_WRAPPER_PATH} libcef_dll_wrapper)

set(SABINE_HOST_SOURCES
  main.cc
  entry.h
  app/app.cc
  app/app.h
  common/json.cc
  common/json.h
  guest/input.cc
  guest/input.h
  guest/manager.cc
  guest/manager.h
  osr/handler.cc
  osr/handler.h
  osr/bridge.cc
  osr/callbacks.cc
  osr/downloads.cc
  osr/drag.cc
  osr/guest/commands.cc
  osr/guest/lifecycle.cc
  osr/ime.cc
  osr/ime.h
  osr/input.cc
  osr/screen.cc
  osr/screen.h
  osr/tasks.cc
  osr/tasks.h
  osr/transport.cc
  osr/utilities.cc
  osr/utilities.h
  osr/accelerated/paint.cc
  osr/accelerated/paint.h
  osr/accelerated/protocol.cc
  osr/accelerated/protocol.h
  osr/accelerated/windows/d3d11_copy.cc
  osr/accelerated/windows/d3d11_copy.h
)

if(OS_MAC)
  list(APPEND SABINE_HOST_SOURCES main_mac.mm)
endif()

if(NOT OS_WINDOWS)
  list(REMOVE_ITEM SABINE_HOST_SOURCES
    osr/accelerated/windows/d3d11_copy.cc
    osr/accelerated/windows/d3d11_copy.h
  )
endif()

if(OS_WINDOWS)
  add_executable(sabine-host WIN32 ${SABINE_HOST_SOURCES})
elseif(OS_MAC)
  add_executable(sabine-host MACOSX_BUNDLE ${SABINE_HOST_SOURCES})
else()
  add_executable(sabine-host ${SABINE_HOST_SOURCES})
endif()

SET_EXECUTABLE_TARGET_PROPERTIES(sabine-host)
target_include_directories(sabine-host PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}"
  "${CEF_ROOT}"
)
add_dependencies(sabine-host libcef_dll_wrapper)
if(OS_MAC)
  target_link_libraries(sabine-host libcef_dll_wrapper ${CEF_STANDARD_LIBS})
else()
  ADD_LOGICAL_TARGET("libcef_lib" "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")
  target_link_libraries(sabine-host libcef_lib libcef_dll_wrapper ${CEF_STANDARD_LIBS})
endif()
if(OS_WINDOWS)
  target_link_libraries(sabine-host ws2_32 d3d11 d3d12 dxgi)
endif()
if(OS_LINUX)
  set_target_properties(sabine-host PROPERTIES INSTALL_RPATH "$ORIGIN")
  set_target_properties(sabine-host PROPERTIES BUILD_WITH_INSTALL_RPATH TRUE)
endif()
if(OS_MAC)
  set(EXECUTABLE_NAME "sabine-host")
  set(PRODUCT_NAME "sabine-host")
  set(PRODUCT_BUNDLE_IDENTIFIER "dev.sabine.host")
  set_target_properties(sabine-host PROPERTIES
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_SOURCE_DIR}/mac/Info.plist.in"
  )
endif()
if(NOT SABINE_HOST_OUTPUT_DIR)
  set(SABINE_HOST_OUTPUT_DIR "${CEF_ROOT}/Release")
endif()
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${SABINE_HOST_OUTPUT_DIR}")
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY_DEBUG "${SABINE_HOST_OUTPUT_DIR}")
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY_RELEASE "${SABINE_HOST_OUTPUT_DIR}")

if(OS_MAC)
  set(SABINE_HOST_APP "${SABINE_HOST_OUTPUT_DIR}/sabine-host.app")
  COPY_MAC_FRAMEWORK("sabine-host" "${CEF_BINARY_DIR}" "${SABINE_HOST_APP}")

  set(_sabine_helper_index 0)
  foreach(_suffix_list ${CEF_HELPER_APP_SUFFIXES})
    string(REPLACE ":" ";" _suffix_list ${_suffix_list})
    list(GET _suffix_list 0 _name_suffix)
    list(GET _suffix_list 2 _plist_suffix)
    set(_helper_name "sabine-host Helper${_name_suffix}")
    set(_helper_app "${SABINE_HOST_APP}/Contents/Frameworks/${_helper_name}.app")
    set(_helper_plist "${CMAKE_CURRENT_BINARY_DIR}/sabine-host-helper-${_sabine_helper_index}.plist")
    math(EXPR _sabine_helper_index "${_sabine_helper_index} + 1")
    file(WRITE "${_helper_plist}"
      "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
      "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
      "<plist version=\"1.0\"><dict>\n"
      "<key>CFBundleExecutable</key><string>${_helper_name}</string>\n"
      "<key>CFBundleIdentifier</key><string>dev.sabine.host.helper${_plist_suffix}</string>\n"
      "<key>CFBundleName</key><string>${_helper_name}</string>\n"
      "<key>CFBundlePackageType</key><string>APPL</string>\n"
      "<key>LSBackgroundOnly</key><true/>\n"
      "</dict></plist>\n")
    add_custom_command(TARGET sabine-host POST_BUILD
      COMMAND ${CMAKE_COMMAND} -E make_directory "${_helper_app}/Contents/MacOS"
      COMMAND ${CMAKE_COMMAND} -E copy "$<TARGET_FILE:sabine-host>" "${_helper_app}/Contents/MacOS/${_helper_name}"
      COMMAND ${CMAKE_COMMAND} -E copy "${_helper_plist}" "${_helper_app}/Contents/Info.plist"
      VERBATIM)
  endforeach()
else()
  COPY_FILES("sabine-host" "${CEF_BINARY_FILES}" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
  COPY_FILES("sabine-host" "${CEF_RESOURCE_FILES}" "${CEF_RESOURCE_DIR}" "${CEF_ROOT}/Release")
endif()

if(OS_LINUX)
  if(EXISTS "${CEF_BINARY_DIR}/libminigbm.so")
    COPY_FILES("sabine-host" "libminigbm.so" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
  endif()
  SET_LINUX_SUID_PERMISSIONS("sabine-host" "${CEF_ROOT}/Release/chrome-sandbox")
endif()
