cmake_minimum_required(VERSION 3.21)

project(sabine_cef_host)

set(CMAKE_CONFIGURATION_TYPES Debug Release)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

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

add_subdirectory(${CEF_LIBCEF_DLL_WRAPPER_PATH} libcef_dll_wrapper)
ADD_LOGICAL_TARGET("libcef_lib" "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")

set(SABINE_HOST_SOURCES
  main.cc
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

if(NOT OS_WINDOWS)
  list(REMOVE_ITEM SABINE_HOST_SOURCES
    osr/accelerated/windows/d3d11_copy.cc
    osr/accelerated/windows/d3d11_copy.h
  )
endif()

if(OS_WINDOWS)
  add_executable(sabine-host WIN32 ${SABINE_HOST_SOURCES})
elseif(OS_MAC)
  add_executable(sabine-host ${SABINE_HOST_SOURCES})
else()
  add_executable(sabine-host ${SABINE_HOST_SOURCES})
endif()

SET_EXECUTABLE_TARGET_PROPERTIES(sabine-host)
target_include_directories(sabine-host PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}"
  "${CEF_ROOT}"
)
add_dependencies(sabine-host libcef_dll_wrapper)
target_link_libraries(sabine-host libcef_lib libcef_dll_wrapper ${CEF_STANDARD_LIBS})
if(OS_WINDOWS)
  target_link_libraries(sabine-host ws2_32 d3d11 d3d12 dxgi)
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
