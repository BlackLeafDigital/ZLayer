cmake_minimum_required(VERSION 3.20)
project(zlayer_sdk C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# WASI SDK settings
if(NOT DEFINED WASI_SDK_PREFIX)
    set(WASI_SDK_PREFIX "/opt/wasi-sdk")
endif()

# Library target
add_library(zlayer_sdk STATIC
    src/zlayer.c
)

target_include_directories(zlayer_sdk PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

# Install rules
install(TARGETS zlayer_sdk
    ARCHIVE DESTINATION lib
)

install(FILES include/zlayer.h
    DESTINATION include
)

# Example target (optional)
option(BUILD_EXAMPLES "Build example plugins" OFF)
if(BUILD_EXAMPLES)
    add_subdirectory(examples)
endif()
