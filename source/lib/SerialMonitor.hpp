
// #pragma once

// #include <string>
// #include <iostream>
// #include <boost/asio.hpp>
// #include <bitset>
// #include <map>

// using namespace std;
// //
// //const PULSE = 40;
// //
// //const X0_LOW = 60;
// //const X0_HIGH = 72;
// //
// //const Y0_LOW = 71;
// //const Y0_HIGH = 82;
// //
// //const X1_LOW = 81;
// //const X1_HIGH = 92;
// //
// //const Y1_LOW = 91;
// //const Y1_HIGH = 102;
// //
// //const X0_SKIP_LOW = 101;
// //const X0_SKIP_HIGH = 114;
// //
// //const Y0_SKIP_LOW = 113;
// //const Y0_SKIP_HIGH = 124;
// //
// //const X1_SKIP_LOW = 123;
// //const X1_SKIP_HIGH = 135;
// //
// //const Y1_SKIP_LOW = 134;
// //const Y1_SKIP_HIGH = 148;

// enum class PulseType : uint8_t
// {
// 	interval,
// 	x0,
// 	y0,
// 	x1,
// 	y1,
// 	x0_skip,
// 	y0_skip,
// 	x1_skip,
// 	y1_skip,
// 	laser,
// };

// map<PulseType, string> pulse_type_to_string = {
// 	{ PulseType::interval , "interval" },
// 	{ PulseType::x0 , "x0" },
// 	{ PulseType::y0 , "y0" },
// 	{ PulseType::x1 , "x1" },
// 	{ PulseType::y1 , "y1" },
// 	{ PulseType::x0_skip , "x0_skip" },
// 	{ PulseType::y0_skip , "y0_skip" },
// 	{ PulseType::x1_skip , "x1_skip" },
// 	{ PulseType::y1_skip , "y1_skip" },
// 	{ PulseType::laser , "laser" }
// };

// map<PulseType, string> simlpe_pulse_type_to_string = {
// 	{ PulseType::interval , "interval" },
// 	{ PulseType::x0 , "A" },
// 	{ PulseType::y0 , "A" },
// 	{ PulseType::x1 , "B" },
// 	{ PulseType::y1 , "B" },
// 	{ PulseType::x0_skip , "A" },
// 	{ PulseType::y0_skip , "A" },
// 	{ PulseType::x1_skip , "B" },
// 	{ PulseType::y1_skip , "B" },
// 	{ PulseType::laser , "laser" }
// };

// struct PulseBlock {
// 	PulseType first : 4;
// 	PulseType second : 4;
// };

// struct PulsePacket {
// 	PulseBlock data[8];
// };

// class SerialMonitor {
// public:
//     /**
//      * Constructor.
//      * \param port device name, example "/dev/ttyUSB0" or "COM4"
//      * \param baud_rate communication speed, example 9600 or 115200
//      * \throws boost::system::system_error if cannot open the
//      * serial device
//      */
//     SerialMonitor(std::string port, unsigned int baud_rate)
//     : io(), serial(io, port)
//     {
//         serial.set_option(boost::asio::serial_port_base::baud_rate(baud_rate));
//     }
    
//     /**
//      * Write a string to the serial device.
//      * \param s string to write
//      * \throws boost::system::system_error on failure
//      */
//     void writeString(std::string s)
//     {
//         boost::asio::write(serial,boost::asio::buffer(s.c_str(),s.size()));
//     }
    
//     /**
//      * Blocks until a line is received from the serial device.
//      * Eventual '\n' or '\r\n' characters at the end of the string are removed.
//      * \return a string containing the received line
//      * \throws boost::system::system_error on failure
//      */

// 	using DataPack = uint64_t;

// 	static constexpr uint16_t PACKET_SIZE = sizeof(DataPack);

// 	union Data {
// 		uint8_t bytes[PACKET_SIZE];
// 		DataPack number;
// 	};

//     std::string readLine()
//     {
// 		static uint16_t delays[20] = { 0 };

// 		//std::thread([&] {
// 		//	while (1)
// 		//	{
// 		//		std::this_thread::sleep_for(1s);
// 		//		for (int i = 0; i < 20; i++)
// 		//			cout << (uint64_t)delays[i] << " ";
// 		//		cout << endl;
// 		//	}
// 		//}).detach();

// #define NUMBER true
// #define BYTES false
// #define SINGLE_BYTE false
// #define SINGLE_VALUE false
// #define PULSES false
// #define DELAY false

//         using namespace boost;
// 		uint8_t c;
//         std::string result;
// 		static uint8_t filled_bytes = 0;
// 		static DataPack prev_value = 0;
//         for(;;)
//         {
//             asio::read(serial,asio::buffer(&c,1));

// #if SINGLE_BYTE
// 			for (int i = 0; i < 8; i++)
// 				cout << bool(c & 1 << i);
// 			cout << endl;
// #endif
// #if SINGLE_VALUE
// 			cout << (uint64_t)c << endl;
// #endif

// 			static Data data;

// 			if (filled_bytes < PACKET_SIZE)
// 			{
// 				//cout << "byte n: " << (uint64_t)filled_bytes << " " << (uint64_t)c << " " << std::bitset<8>(c) << endl;
// 				data.bytes[filled_bytes] = c;
// 				filled_bytes++;
// 			}
// 			else
// 			{

// #if NUMBER
// 				cout <<
// 					data.number// / 2000
// 					<< endl;
// #endif

// #if BYTES
// 				for (int i = sizeof(DataPack) * 8 - 1; i > 0; i--) {
// 					cout << (bool)(data.number & (static_cast<DataPack>(1) << i));
// 					if ((i) % 8 == 0)
// 						cout << " ";
// 				}
// 				cout << endl;
// 				cout << endl;
// #endif

// #if PULSES
// 				PulsePacket packet;
// 				memcpy(&packet, &data.number, sizeof(PulsePacket));
// 				for (int i = 0; i < PACKET_SIZE; i++)
// 				{
// 					cout << pulse_type_to_string[packet.data[i].first] << " ";
// 					cout << pulse_type_to_string[packet.data[i].second] << " ";
// 				}
// 				cout << endl;
// #endif

// #if DELAY
// 				auto delay = data.number - prev_value;
// 				cout << delay << endl;
// 				prev_value = data.number;
// #endif
// 		/*		if (delay < 200)
// 					delays[delay]++;*/

// 				data.number = 0;
// 				filled_bytes = 0;
// 			}
//         }

//         return result;
//     }
    
// private:
//     boost::asio::io_service io;
//     boost::asio::serial_port serial;
    
// };
