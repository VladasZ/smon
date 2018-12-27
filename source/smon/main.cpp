
#pragma comment(lib, "ws2_32")

#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"
#include "ExceptionCatch.hpp"
#include "Log.hpp"

using namespace std;


int main() {


    cout << "Helloy1" << endl;


    cout << "Helloy2" << endl;


    SerialMonitor smon("/dev/ttyACM0", 57600);


    smon.readLine();



	cout << "Helloy" << endl;


	return 0;
}
