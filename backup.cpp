
#include <iostream>
#include <map>

#include "mbed.h"
#include "Bitwise.hpp"


using namespace std;


const uint16_t PULSE = 40;

const uint16_t X0_LOW = 60;
const uint16_t X0_HIGH = 72;

const uint16_t Y0_LOW = 71;
const uint16_t Y0_HIGH = 82;

const uint16_t X1_LOW = 81;
const uint16_t X1_HIGH = 92;

const uint16_t Y1_LOW = 91;
const uint16_t Y1_HIGH = 102;

const uint16_t X0_SKIP_LOW = 101;
const uint16_t X0_SKIP_HIGH = 114;

const uint16_t Y0_SKIP_LOW = 113;
const uint16_t Y0_SKIP_HIGH = 124;

const uint16_t X1_SKIP_LOW = 123;
const uint16_t X1_SKIP_HIGH = 135;

const uint16_t Y1_SKIP_LOW = 134;
const uint16_t Y1_SKIP_HIGH = 148;

enum class SynchroPulseType : uint8_t {
	in = 0,
	X0 = 1,
	Y0 = 2,
	X1 = 3,
	Y1 = 4,
	x0 = 5,
	y0 = 6,
	x1 = 7,
	y1 = 8,
	ls = 9
};

SynchroPulseType synchro_pulse_from_duration(uint16_t duration) {
	if (duration < PULSE) return SynchroPulseType::ls;
	if (duration > X0_LOW && duration <= X0_HIGH) return SynchroPulseType::X0;
	if (duration > Y0_LOW && duration <= Y0_HIGH) return SynchroPulseType::Y0;
	if (duration > X1_LOW && duration <= X1_HIGH) return SynchroPulseType::X1;
	if (duration > Y1_LOW && duration <= Y1_HIGH) return SynchroPulseType::Y1;
	if (duration > X0_SKIP_LOW && duration <= X0_SKIP_HIGH) return SynchroPulseType::x0;
	if (duration > Y0_SKIP_LOW && duration <= Y0_SKIP_HIGH) return SynchroPulseType::y0;
	if (duration > X1_SKIP_LOW && duration <= X1_SKIP_HIGH) return SynchroPulseType::x1;
	if (duration > Y1_SKIP_LOW && duration <= Y1_SKIP_HIGH) return SynchroPulseType::y1;
	return SynchroPulseType::in;
}

map <SynchroPulseType, string> syncho_pulse_to_string = { 
    {SynchroPulseType::in , "in" },
	{SynchroPulseType::X0 , "X0" },
	{SynchroPulseType::Y0 , "Y0" },
	{SynchroPulseType::X1 , "X1" },
	{SynchroPulseType::Y1 , "Y1" },
	{SynchroPulseType::x0 , "x0" },
	{SynchroPulseType::y0 , "y0" },
	{SynchroPulseType::x1 , "x1" },
	{SynchroPulseType::y1 , "y1" },
	{SynchroPulseType::ls , "ls" }
};

class Pin : public DigitalIn {
public:

	using DigitalIn::DigitalIn;

	void wait_for_low() { while (read()); }
	void wait_for_high() { while (!read()); }

	void wait_for_change() {
		auto current = read();
		while (read() == current);
	}
};

Pin transmission(D4);
Pin data_pin(D5);
Pin _clock(D6);
DigitalOut busy(D7);
Serial pc(USBTX, USBRX); // tx, rx

#define NUMBER false
#define BYTES false
#define NUMS_4 false
#define NUMS_8 false
#define NUMS_16 false
#define SINGLE_BYTE false
#define SINGLE_VALUE false
#define DELAY false
#define CHECK_SEQUENCE false

#define SINGLE_PULSE false
#define PULSE_TYPES_OUTPUT false
#define PULSES true

#define GET_PACK true

#define PACK_SIZE 5
#define PACK_NUMBERS false

#define PART_SIZE 64
#define PARTS_COUNT 1
#define PACKET_SIZE (PART_SIZE * PARTS_COUNT) / 8

int received = 0;

class Pulse {
	uint16_t _duration;
public:
	Pulse() = default;
	constexpr Pulse(uint16_t data) : _duration(data) { }

	constexpr bool station() const { return _duration & 1 << 13; }
	auto duration() const {
		uint16_t result = _duration;
		result &= ~(1 << 15);
		result &= ~(1 << 14);
		result &= ~(1 << 13);

#if PULSE_TYPES_OUTPUT

		if (result > 9) {
			return std::string("fa");
		}
		return syncho_pulse_to_string[static_cast<SynchroPulseType>(result)];
	
#else
		return result;
#endif
}
	std::string to_string(const std::string& axis) const {
		if (_duration == 0)
			return "0 0 00";

#if PULSE_TYPES_OUTPUT
		return station_to_string(station()) + " " + axis + " " + duration();
#else
		//return station_to_string(station()) + " " + axis + " " + std::to_string(duration());
		return std::to_string(duration());
#endif
	}

	static std::string station_to_string(bool station) { return station ? "A" : "B"; }
};

class CycleData {

public:

	uint8_t id;
	Pulse a1_pulse;
	Pulse a2_pulse;
	Pulse b1_pulse;
	Pulse b2_pulse;

	constexpr CycleData(uint64_t data) :
		a1_pulse(bitwise::get_part< 0, 14>(data)),
		a2_pulse(bitwise::get_part<14, 28>(data)),
		b1_pulse(bitwise::get_part<28, 42>(data)),
		b2_pulse(bitwise::get_part<42, 56>(data)),
		id(bitwise::get_part<56, 64>(data))
	{ }

	std::string to_string() {
		return std::string()// + "ID: " + std::to_string(id) + " " +
			+
			a1_pulse.to_string("X") + ", " +
			a2_pulse.to_string("Y") + ", " +
			b1_pulse.to_string("X") + ", " +
			b2_pulse.to_string("Y") + " ";
	}
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
	uint8_t _data[size];
	void clear() { memset(_data, 0, size); }
};

using DataPack = Base_DataPack<PACKET_SIZE>;

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
	uint64_t int64;
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

	static uint64_t bytes_count = 1;
	if (bytes_count++ % 8 == 0)
		cout << endl;

#endif

#if SINGLE_VALUE
	cout << (uint64_t)byte << endl;
#endif

	data_buffer.bytes[test.filled_bytes++] = byte;

	if (test.filled_bytes == PACKET_SIZE)
	{
		busy.write(1);

#if NUMBER
		auto value = bitwise::to_number(data_buffer.number);

		PRINT_LU(bitwise::set_byte(value, 13, 0));
		//PRINT_LU(data_buffer.number);
#endif

#if BYTES && !GET_PACK
		PRINT((bitwise::to_string(data_buffer) + " \n").c_str());
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
			result &= ~(1 << 13);
			PRINT_D(result);
		}
		PRINT("\n");
#endif

#if PULSES && !GET_PACK
		auto cycle_data = CycleData(data_buffer.int64[0]);
		PRINT((cycle_data.to_string() + "\n").c_str());
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

#if BYTES
				cout << bitwise::to_string(pack[i].int64) << endl;
#endif

#if PULSES 
				auto cycle_data = CycleData(pack[i].int64);
				cout << cycle_data.to_string() << endl;
#endif

#if PACK_NUMBERS
				PRINT_LU(pack[i].int64);
#endif


			}

			cout << "\n" << received++;
		}
#endif

		data_buffer.number.clear();
		test.filled_bytes = 0;

		busy.write(0);
	}
}


int main()
{
	pc.baud(57600);

	while (true)
	{
		uint8_t pending_byte = 0;
		int bit_index = 0;


		// pc.printf("rig");
		// cout << "sopokok" << endl;


		//		wait(1);
		transmission.wait_for_high();

		pending_byte |= data_pin.read() << bit_index;
		_clock.wait_for_change();
		bit_index++;

		while (transmission.read()) {
			pending_byte |= data_pin.read() << bit_index;
			_clock.wait_for_change();
			bit_index++;
		}

		//pending_byte = 7;

		//pc.write(&pending_byte, 1, 0, 0);

		//pc.printf((char)pending_byte);

		process_byte(pending_byte);
	}
}
