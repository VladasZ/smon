
#pragma once


#include <boost/asio.hpp>

#include "Log.hpp"

using namespace std;

class SerialMonitor {
public:

    SerialMonitor(std::string port, unsigned int baud_rate)
    : io(), serial(io, port)
    {
        serial.set_option(boost::asio::serial_port_base::baud_rate(baud_rate));
    }

    void writeString(std::string s)
    {
        boost::asio::write(serial, boost::asio::buffer(s.c_str(),s.size()));
    }
    
    std::string readLine()
    {
        using namespace boost;

        void* data = malloc(1000);

		uint8_t c;
        for(;;)
        {
            asio::read(serial, asio::buffer(data,1000));

            cout << static_cast<const char*>(data) << endl;
        }

        return "";
    }
    
private:
    boost::asio::io_service io;
    boost::asio::serial_port serial;
    
};
