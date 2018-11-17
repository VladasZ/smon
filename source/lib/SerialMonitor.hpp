
#pragma once

#include <boost/asio.hpp>


#include <string>
#include <iostream>
#include <bitset>
#include <map>

using namespace std;

#if ARM_CHIP

#include "mbed.h"

Serial pc(USBTX, USBRX); // tx, rx

#define PRINT(val) pc.printf(val)
#define PRINT_LU(val) pc.printf("%lu ", (val))
#define PRINT_D(val) pc.printf("%d ", (val))
#define PRINT_S(val) pc.printf("%s ", (val))
#else
#define PRINT(val) std::cout << (val)
#define PRINT_LU(val) std::cout << " " << (*static_cast<uint64_t *>((uint64_t *)((void *)&val)))
#define PRINT_D(val) std::cout << " " << (val)
#define PRINT_S(val) std::cout << " " << (val)
#endif

#define NUMBER true
#define BYTES false
#define NUMS_4 false
#define NUMS_8 false
#define NUMS_16 false
#define SINGLE_BYTE false
#define SINGLE_VALUE false
#define SINGLE_PULSE false
#define PULSES false
#define DELAY false
#define GET_PACK false
#define CHECK_SEQUENCE false
#define REAL_AXIS false

#define PACK_SIZE 16

#define PART_SIZE 16
#define PARTS_COUNT 4
#define PACKET_SIZE (PART_SIZE * PARTS_COUNT) / 8

int received = 0;

#if !REAL_AXIS
static bool axis;
#endif

class PulseData {
	uint16_t _duration;
public:
	PulseData() = default;
	PulseData(uint16_t data) : _duration(data) { }
#if REAL_AXIS
	bool axis() const { return _duration & 1 << 15; }
#endif
	bool station() const { return _duration & 1 << 14; }
	uint16_t duration() const {
		auto result = _duration;
		result &= ~(1 << 15);
		result &= ~(1 << 14);
		return result;
	}
	std::string to_string() const {
#if REAL_AXIS
		return station_to_string(station()) + " " + axis_to_string(axis()) + " " + std::to_string(duration());
#else
		return station_to_string(station()) + " " + axis_to_string(axis) + " " + std::to_string(duration());
#endif
	}
	static std::string axis_to_string(bool axis) { return axis ? "X" : "Y"; }
	static std::string station_to_string(bool station) { return station ? "A" : "B"; }
};

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


template <uint8_t size>
class Base_DataPack {
public:
	char _data[size];
	void clear() { memset(_data, 0, size); }
};

using DataPack = Base_DataPack<PACKET_SIZE * 8>;

struct PulseBlock {
	PulseType first : 4;
	PulseType second : 4;
};

struct PulsePacket {
	PulseBlock data[8];
};

union Data {
	uint8_t bytes[PACKET_SIZE];
	uint16_t int16[PACKET_SIZE / 2];
	PulseData pulses[PACKET_SIZE / 2];
	uint64_t int64[PACKET_SIZE / 8];
	DataPack number;
};

#if GET_PACK
Data pack[PACK_SIZE];
int pack_index = 0;
#endif

Data data_buffer;

struct
{
	bool transmission;
	bool data;
	bool _clock;
	bool ok;
	uint8_t number;
	uint8_t filled_bytes = 0;
	int bytes_count = 0;

}test;


void process_byte(uint8_t byte) {

#if SINGLE_BYTE
	for (int i = 0; i < 8; i++)
		cout << bool(byte & 1 << i);
	cout << endl;
#endif

#if SINGLE_VALUE
	cout << (uint64_t)byte << endl;
#endif

	data_buffer.bytes[test.filled_bytes++] = byte;

	if (test.filled_bytes == PACKET_SIZE)
	{
		//busy.write(1);

#if NUMBER
		PRINT_LU(data_buffer.number);
		PRINT("\n");
#endif

#if BYTES

		for (int i = 0; i < PACKET_SIZE; i++) {
			for (int j = 0; j < 8; j++) {
				PRINT_D((bool)(data_buffer.bytes[i] & j));
			}
			PRINT(" ");
		}
		PRINT("\n");


		//for (int i = sizeof(DataPack) * 8 - 1; i >= 0; i--) {
		//	pc.printf("%d", (bool)(data_buffer.number & (static_cast<DataPack>(1) << i)));
		//	if ((i) % 8 == 0)
		//		pc.printf(" ");
		//}
		//pc.printf("\n");
#endif

#if NUMS_4
		for (int i = 0; i < PACKET_SIZE; i++) {
			PRINT_D(static_cast<uint8_t>(data_buffer.pulses[i].first));
			PRINT_D(static_cast<uint8_t>(data_buffer.pulses[i].second));
		}
		PRINT("\n");
#endif

#if NUMS_8
		for (int i = 0; i < PACKET_SIZE; i++) {
			PRINT_D(static_cast<uint8_t>(data_buffer.bytes[i]));
		}
		PRINT("\n");
#endif

#if NUMS_16
		for (int i = 0; i < PACKET_SIZE / 2; i++) {
			auto result = data_buffer.int16[i];
			result &= ~(1 << 15);
			result &= ~(1 << 14);
			PRINT_D(result);
		}
		PRINT("\n");
#endif

#if PULSES
		for (int i = 0; i < PACKET_SIZE / 2; i++) {
#if !REAL_AXIS
			axis = i % 2 == 0;
#endif
			PRINT_S(data_buffer.pulses[i].to_string().c_str());
		}
		PRINT("\n");
#endif

#if SINGLE_PULSE
		PRINT(pulse_type_to_string[static_cast<PulseType>(data_buffer.number)].c_str());
		PRINT("\n");
#endif

#if DELAY
		auto delay = data.number - prev_value;
		cout << delay << endl;
		prev_value = data.number;
#endif

#if GET_PACK

		memcpy(&pack[pack_index++], &data_buffer.number, PACKET_SIZE);

		if (pack_index == PACK_SIZE)
		{
			pack_index = 0;

			for (int i = 0; i < PACK_SIZE; i++) {

#if CHECK_SEQUENCE
				/*	if (i > 0 && pack[i] != (pack[i - 1] + 1)) {
						pc.printf("fail:\n");
					}*/
#endif

				pc.printf("%d\n", pack->int64[0]);
				pc.printf("%d\n", pack->int64[1]);
			}

			pc.printf("received:%d\n", received++);
		}
#endif

		data_buffer.number.clear();
		test.filled_bytes = 0;

		//busy.write(0);
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
        for(;;)
        {
            asio::read(serial,asio::buffer(&c,1));
			process_byte(c);
        }

        return "";
    }
    
private:
    boost::asio::io_service io;
    boost::asio::serial_port serial;
    
};
