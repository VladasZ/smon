
#pragma comment(lib, "ws2_32")

#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"

using namespace std;


SerialMonitor smon("COM5", 250000);

int main() {
  
	cout << "Helloy" << endl;

	smon.readLine();

	return 0;
}
