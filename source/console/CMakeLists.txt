cmake_minimum_required(VERSION 3.8 FATAL_ERROR)

project(console)

add_catalog_recursive(${PROJECT_SOURCE_DIR} / SOURCE)
add_executable(${PROJECT_NAME} ${SOURCE})

link_project(lib)
