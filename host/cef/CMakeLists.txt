# Minimal glint-cef.exe — windowless OSR helper for Electron host.
cmake_minimum_required(VERSION 3.21)
project(glint_cef LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT DEFINED CEF_ROOT OR CEF_ROOT STREQUAL "")
  get_filename_component(CEF_ROOT "${CMAKE_SOURCE_DIR}/../../third_party/cef/current" ABSOLUTE)
endif()
if(NOT EXISTS "${CEF_ROOT}/cmake/FindCEF.cmake")
  message(FATAL_ERROR "CEF not found at ${CEF_ROOT}. Run: powershell -File scripts/fetch-cef.ps1")
endif()

set(CMAKE_MODULE_PATH ${CMAKE_MODULE_PATH} "${CEF_ROOT}/cmake")
find_package(CEF REQUIRED)

# libcef_dll_wrapper
add_subdirectory(${CEF_LIBCEF_DLL_WRAPPER_PATH} libcef_dll_wrapper)

# Protobuf runtime + protoc (matches generated code under src/ipc/gen).
# SSOT schema: core/overlay/cef-protocol/proto/cef_ipc.proto
include(FetchContent)
set(protobuf_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_CONFORMANCE OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_PROTOC_BINARIES ON CACHE BOOL "" FORCE)
set(protobuf_MSVC_STATIC_RUNTIME ON CACHE BOOL "" FORCE)
set(protobuf_WITH_ZLIB OFF CACHE BOOL "" FORCE)
set(protobuf_INSTALL OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  protobuf
  URL https://github.com/protocolbuffers/protobuf/releases/download/v21.12/protobuf-all-21.12.tar.gz
  DOWNLOAD_EXTRACT_TIMESTAMP TRUE
  SOURCE_SUBDIR cmake
)
FetchContent_MakeAvailable(protobuf)

set(GO_PROTO_DIR
  "${CMAKE_SOURCE_DIR}/../../core/overlay/cef-protocol/proto")
get_filename_component(GO_PROTO_DIR "${GO_PROTO_DIR}" ABSOLUTE)
set(GO_PROTO_SSOT "${GO_PROTO_DIR}/cef_ipc.proto")
set(GO_PROTO_GEN_DIR "${CMAKE_CURRENT_SOURCE_DIR}/src/ipc/gen")
set(GO_PROTO_CC "${GO_PROTO_GEN_DIR}/cef_ipc.pb.cc")
set(GO_PROTO_H "${GO_PROTO_GEN_DIR}/cef_ipc.pb.h")

add_custom_command(
  OUTPUT "${GO_PROTO_CC}" "${GO_PROTO_H}"
  COMMAND ${CMAKE_COMMAND} -E make_directory "${GO_PROTO_GEN_DIR}"
  # Leading colon: empty plugin options so Windows drive letters are not parsed as options.
  COMMAND $<TARGET_FILE:protoc>
          -I "${GO_PROTO_DIR}"
          "--cpp_out=:${GO_PROTO_GEN_DIR}"
          "${GO_PROTO_SSOT}"
  DEPENDS "${GO_PROTO_SSOT}" protoc
  COMMENT "Generating cef_ipc.pb.cc/h from cef-protocol SSOT"
  VERBATIM
)

set(CEF_TARGET "glint-cef")
ADD_LOGICAL_TARGET("libcef_lib" "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")
SET_CEF_TARGET_OUT_DIR()

set(GO_CEF_SRCS
  src/main.cpp
  src/ipc_server.cpp
  src/ipc_dispatch.cpp
  src/browser_app.cpp
  src/plugin_scheme.cpp
  src/osr_client.cpp
  src/shared_tex.cpp
  "${GO_PROTO_CC}"
)

add_executable(${CEF_TARGET} WIN32 ${GO_CEF_SRCS})
SET_EXECUTABLE_TARGET_PROPERTIES(${CEF_TARGET})
add_dependencies(${CEF_TARGET} libcef_dll_wrapper)
target_include_directories(${CEF_TARGET} PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/src"
  "${GO_PROTO_GEN_DIR}"
)
target_link_libraries(${CEF_TARGET}
  libcef_lib
  libcef_dll_wrapper
  ${CEF_STANDARD_LIBS}
  d3d11
  dxgi
  shell32
  protobuf::libprotobuf
)
if(MSVC)
  target_compile_options(${CEF_TARGET} PRIVATE /bigobj)
endif()

# Windows: delay-load + manifest helpers from CEF macros
if(OS_WINDOWS)
  set_target_properties(${CEF_TARGET} PROPERTIES RUNTIME_OUTPUT_DIRECTORY ${CEF_TARGET_OUT_DIR})
  COPY_FILES("${CEF_TARGET}" "${CEF_BINARY_FILES}" "${CEF_BINARY_DIR}" "${CEF_TARGET_OUT_DIR}")
  COPY_FILES("${CEF_TARGET}" "${CEF_RESOURCE_FILES}" "${CEF_RESOURCE_DIR}" "${CEF_TARGET_OUT_DIR}")
endif()

set(GO_CEF_STAGE "${CMAKE_SOURCE_DIR}/../../host/native/cef")
add_custom_command(TARGET ${CEF_TARGET} POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "${GO_CEF_STAGE}"
  COMMAND ${CMAKE_COMMAND} -E copy_directory
    "${CEF_TARGET_OUT_DIR}" "${GO_CEF_STAGE}"
  COMMENT "Staging glint-cef to host/native/cef"
)
