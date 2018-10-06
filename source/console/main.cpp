
#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"

using namespace std;

//SerialMonitor serial("/dev/cu.usbmodemFA131", 9600);
SerialMonitor serial("COM10", 50000000);

int main() {

	//PulseBlock block;

	//block.first = PulseType::x0_skip;
	//block.second = PulseType::y0_skip;

	//cout << pulse_type_to_string[block.first] << endl;
	//cout << pulse_type_to_string[block.second] << endl;

	//cout << sizeof(PulseBlock) << endl;
	//cout << sizeof(PulsePacket) << endl;

	//return 0;
	//int ind = 0;

	//while (ind < 100)
	//{
	//	auto ind1 = ind;
	//	ind += 7;
	//	auto ind2 = ind;
	//	ind += 1;
	//	cout << ind2 << " : " << ind1 << endl;
	//}

   // std::thread([]{
		serial.readLine();
  //  }).detach();
    
    //while (1)
    //{
    //    string message;
    //    
    //    cin >> message;
    //    
    //    serial.writeString(message);
    //    
    //    cout << "Cout " << message << endl;

    //}
    
    //cout << serial.readLine() << endl;
    
	return 0;
}
