cmake_minimum_required(VERSION 3.9.2 FATAL_ERROR)

include(${CMAKE_BINARY_DIR}/build_info.cmake)
include(${CMAKE_BINARY_DIR}/${BUILD_INFO})
include(${BUILD_SCRIPT_PATH})

set(CMAKE_CXX_STANDARD 17)

project(smon)

add_definitions(-DDEBUG=1)

conan_basic_setup()

add_subdirectory(source/lib)
add_subdirectory(source/smon)
add_subdirectory(source/cpp_utils)
