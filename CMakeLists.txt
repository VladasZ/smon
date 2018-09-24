cmake_minimum_required(VERSION 3.9.2 FATAL_ERROR)

include(${CMAKE_SOURCE_DIR}/config/utils/utils.cmake)

set(CMAKE_CXX_STANDARD 17)

project(cmon)

add_subdirectory(source/lib)
add_subdirectory(source/console)
