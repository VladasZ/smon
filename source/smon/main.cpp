
#pragma comment(lib, "ws2_32")

#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"
#include "ExceptionCatch.hpp"
#include "Log.hpp"

using namespace std;


int main() {

    SerialMonitor smon("/dev/ttyACM0", 57600);

    smon.readLine<int, int>([](auto){ return false; }, [](auto){});

	return 0;
}
