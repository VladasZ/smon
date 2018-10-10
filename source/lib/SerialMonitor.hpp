
#pragma once

#include <string>
#include <iostream>
#include <boost/asio.hpp>
#include <bitset>
#include <map>

using namespace std;
//
//const PULSE = 40;
//
//const X0_LOW = 60;
//const X0_HIGH = 72;
//
//const Y0_LOW = 71;
//const Y0_HIGH = 82;
//
//const X1_LOW = 81;
//const X1_HIGH = 92;
//
//const Y1_LOW = 91;
//const Y1_HIGH = 102;
//
//const X0_SKIP_LOW = 101;
//const X0_SKIP_HIGH = 114;
//
//const Y0_SKIP_LOW = 113;
//const Y0_SKIP_HIGH = 124;
//
//const X1_SKIP_LOW = 123;
//const X1_SKIP_HIGH = 135;
//
//const Y1_SKIP_LOW = 134;
//const Y1_SKIP_HIGH = 148;

enum class PulseType : uint8_t
{
	interval,
	x0,
	y0,
	x1,
	y1,
	x0_skip,
	y0_skip,
	x1_skip,
	y1_skip,
	laser,
};

map<PulseType, string> pulse_type_to_string = {
	{ PulseType::interval , "interval" },
	{ PulseType::x0 , "x0" },
	{ PulseType::y0 , "y0" },
	{ PulseType::x1 , "x1" },
	{ PulseType::y1 , "y1" },
	{ PulseType::x0_skip , "x0_skip" },
	{ PulseType::y0_skip , "y0_skip" },
	{ PulseType::x1_skip , "x1_skip" },
	{ PulseType::y1_skip , "y1_skip" },
	{ PulseType::laser , "laser" }
};

map<PulseType, string> simlpe_pulse_type_to_string = {
	{ PulseType::interval , "interval" },
	{ PulseType::x0 , "A" },
	{ PulseType::y0 , "A" },
	{ PulseType::x1 , "B" },
	{ PulseType::y1 , "B" },
	{ PulseType::x0_skip , "A" },
	{ PulseType::y0_skip , "A" },
	{ PulseType::x1_skip , "B" },
	{ PulseType::y1_skip , "B" },
	{ PulseType::laser , "laser" }
};

struct PulseBlock {
	PulseType first : 4;
	PulseType second : 4;
};

struct PulsePacket {
	PulseBlock data[8];
};

#define NUMBER true
#define BYTES false
#define SINGLE_BYTE false
#define SINGLE_VALUE false
#define PULSES false
#define DELAY false

using DataPack = uint64_t;

static constexpr uint16_t PACKET_SIZE = sizeof(DataPack);

union Data {
	uint8_t bytes[PACKET_SIZE];
	DataPack number;
};

Data data_buffer;
uint8_t filled_bytes = 0;


void process_byte(uint8_t byte) {
#if SINGLE_BYTE
	for (int i = 0; i < 8; i++)
		cout << bool(byte & 1 << i);
	cout << endl;
#endif
#if SINGLE_VALUE
	cout << (uint64_t)byte << endl;
#endif
	if (filled_bytes < PACKET_SIZE)
	{
		data_buffer.bytes[filled_bytes] = byte;
		filled_bytes++;
	}
	else
	{

#if NUMBER
		cout <<
			data_buffer.number
			<< endl;
#endif

#if BYTES
		for (int i = sizeof(DataPack) * 8 - 1; i >= 0; i--) {
			cout << (bool)(data_buffer.number & (static_cast<DataPack>(1) << i));
			if ((i) % 8 == 0)
				cout << " ";
		}
		cout << endl;
#endif

#if PULSES
		PulsePacket packet;
		memcpy(&packet, &data.number, sizeof(PulsePacket));
		for (int i = 0; i < PACKET_SIZE; i++)
		{
			cout << pulse_type_to_string[packet.data_buffer[i].first] << " ";
			cout << pulse_type_to_string[packet.data_buffer[i].second] << " ";
		}
		cout << endl;
#endif

#if DELAY
		auto delay = data.number - prev_value;
		cout << delay << endl;
		prev_value = data.number;
#endif

		data_buffer.number = 0;
		filled_bytes = 0;
	}
}

class SerialMonitor {
public:

    SerialMonitor(std::string port, unsigned int baud_rate)
    : io(), serial(io, port)
    {
        serial.set_option(boost::asio::serial_port_base::baud_rate(baud_rate));
    }

    void writeString(std::string s)
    {
        boost::asio::write(serial,boost::asio::buffer(s.c_str(),s.size()));
    }
    
    std::string readLine()
    {
		static uint16_t delays[20] = { 0 };

        using namespace boost;
		uint8_t c;
        std::string result;
		static DataPack prev_value = 0;
        for(;;)
        {
            asio::read(serial,asio::buffer(&c,1));
			process_byte(c);
        }

        return result;
    }
    
private:
    boost::asio::io_service io;
    boost::asio::serial_port serial;
    
};
