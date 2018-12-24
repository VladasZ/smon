
#pragma once


#include <boost/asio.hpp>

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

			//			process_byte(c);
        }

        return "";
    }
    
private:
    boost::asio::io_service io;
    boost::asio::serial_port serial;
    
};
