
#pragma comment(lib, "ws2_32")

#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"

using namespace std;


int main() {

    SerialMonitor smon("COM5", 250000);

	cout << "Helloy" << endl;

	smon.readLine();

	return 0;
}
