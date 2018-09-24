
#include <iostream>
#include <thread>

#include "SerialMonitor.hpp"

using namespace std;

SerialMonitor serial("/dev/cu.usbmodemFA131", 9600);


int main() {
    

    std::thread([]{
        cout << "IZI ROUUDDD" << serial.readLine() << endl;
        
    }).detach();
    
    while (1)
    {
        string message;
        
        cin >> message;
        
        serial.writeString(message);
        
        cout << "Cout " << message << endl;

    }
    
    //cout << serial.readLine() << endl;
    
	return 0;
}
