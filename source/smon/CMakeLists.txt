cmake_minimum_required(VERSION 3.8 FATAL_ERROR)

setup_lib(smon)

if(WIN32)
  target_link_libraries(${PROJECT_NAME} wsock32 ws2_32)
endif()
