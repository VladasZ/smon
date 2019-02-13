cmake_minimum_required(VERSION 3.8 FATAL_ERROR)

project(lib_smon)

add_catalog_recursive(${PROJECT_SOURCE_DIR} / SOURCE)
add_library(${PROJECT_NAME} ${SOURCE})

if(WIN32)
  target_link_libraries(${PROJECT_NAME} wsock32 ws2_32)
endif()

link_project_at_path(cpp_utils ${cpp_utils_path})

conan_target_link_libraries(${PROJECT_NAME})
