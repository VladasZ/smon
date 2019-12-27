cmake_minimum_required(VERSION 3.9.2 FATAL_ERROR)

if(${CMAKE_SOURCE_DIR} STREQUAL ${CMAKE_CURRENT_SOURCE_DIR})
    include(build/build_tools_generated.cmake)
endif()

project(smon)

setup_conan_if_needed()
setup_deps()

add_subdirectory(source/smon)

if(NOT ${NEEDS_SIGNING})
    add_subdirectory(source/smon_test)
endif()
