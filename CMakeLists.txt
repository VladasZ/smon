cmake_minimum_required(VERSION 3.9.2 FATAL_ERROR)

include(~/.emacs.d/utils/utils.cmake)

include(${CMAKE_BINARY_DIR}/build_info.cmake)
include(${CMAKE_BINARY_DIR}/${BUILD_INFO})

set(CMAKE_CXX_STANDARD 17)

project(smon)

conan_basic_setup()

add_subdirectory(source/lib)
add_subdirectory(source/smon)
