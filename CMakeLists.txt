cmake_minimum_required(VERSION 3.9.2 FATAL_ERROR)

include(~/.emacs.d/utils/utils.cmake)

set(CMAKE_CXX_STANDARD 17)

project(smon)

add_subdirectory(source/lib)
add_subdirectory(source/smon)
