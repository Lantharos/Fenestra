cmake_minimum_required(VERSION 3.21)

project(sabine_cef_host)

set(CMAKE_CONFIGURATION_TYPES Debug Release)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT CEF_ROOT)
  message(FATAL_ERROR "CEF_ROOT is required")
endif()

set(CMAKE_MODULE_PATH ${CMAKE_MODULE_PATH} "${CEF_ROOT}/cmake")
find_package(CEF REQUIRED)

add_subdirectory(${CEF_LIBCEF_DLL_WRAPPER_PATH} libcef_dll_wrapper)
ADD_LOGICAL_TARGET("libcef_lib" "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")

set(SABINE_HOST_SOURCES
  main.cc
  app.cc
  app.h
  guest_input.cc
  guest_input.h
  guest_manager.cc
  guest_manager.h
  json_util.cc
  json_util.h
  osr_handler.cc
  osr_handler.h
  osr_handler_accel_ipc.cc
  osr_handler_accel_ipc.h
  osr_handler_accelerated.cc
  osr_handler_accelerated.h
  osr_handler_bridge.cc
  osr_handler_cef.cc
  osr_handler_downloads.cc
  osr_handler_drag.cc
  osr_handler_guest.cc
  osr_handler_guest_ops.cc
  osr_handler_ime.cc
  osr_handler_ime.h
  osr_handler_input.cc
  osr_handler_ipc.cc
  osr_handler_screen.cc
  osr_handler_screen.h
  osr_handler_util.cc
  osr_handler_util.h
)

if(OS_WINDOWS)
  add_executable(sabine-host WIN32 ${SABINE_HOST_SOURCES})
elseif(OS_MAC)
  add_executable(sabine-host ${SABINE_HOST_SOURCES})
else()
  add_executable(sabine-host ${SABINE_HOST_SOURCES})
endif()

SET_EXECUTABLE_TARGET_PROPERTIES(sabine-host)
target_include_directories(sabine-host PRIVATE "${CEF_ROOT}")
add_dependencies(sabine-host libcef_dll_wrapper)
target_link_libraries(sabine-host libcef_lib libcef_dll_wrapper ${CEF_STANDARD_LIBS})
if(OS_WINDOWS)
  target_link_libraries(sabine-host ws2_32)
endif()
if(OS_MAC)
  target_link_libraries(sabine-host "-framework IOSurface" "-framework CoreFoundation")
endif()

if(OS_LINUX)
  set_target_properties(sabine-host PROPERTIES INSTALL_RPATH "$ORIGIN")
  set_target_properties(sabine-host PROPERTIES BUILD_WITH_INSTALL_RPATH TRUE)
endif()
if(OS_MAC)
  set_target_properties(sabine-host PROPERTIES INSTALL_RPATH "@executable_path")
  set_target_properties(sabine-host PROPERTIES BUILD_WITH_INSTALL_RPATH TRUE)
endif()
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CEF_ROOT}/Release")
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY_DEBUG "${CEF_ROOT}/Release")
set_target_properties(sabine-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY_RELEASE "${CEF_ROOT}/Release")

COPY_FILES("sabine-host" "${CEF_BINARY_FILES}" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
COPY_FILES("sabine-host" "${CEF_RESOURCE_FILES}" "${CEF_RESOURCE_DIR}" "${CEF_ROOT}/Release")

if(OS_LINUX)
  if(EXISTS "${CEF_BINARY_DIR}/libminigbm.so")
    COPY_FILES("sabine-host" "libminigbm.so" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
  endif()
  SET_LINUX_SUID_PERMISSIONS("sabine-host" "${CEF_ROOT}/Release/chrome-sandbox")
endif()
