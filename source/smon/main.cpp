
#pragma comment(lib, "ws2_32")

#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"
#include "ExceptionCatch.hpp"
#include "Log.hpp"

using namespace std;


int main() {

    SerialMonitor smon("/dev/ttyACM0", 57600);

    smon.readLine();

	return 0;
}
