
#pragma once

#include <functional>

#include <boost/asio.hpp>

#include "Log.hpp"

using namespace std;

class SerialMonitor {
public:

    SerialMonitor(std::string port, unsigned int baud_rate) : io(), serial(io, port) {
        serial.set_option(boost::asio::serial_port_base::baud_rate(baud_rate));
    }

    void writeString(std::string s) {
        boost::asio::write(serial, boost::asio::buffer(s.c_str(),s.size()));
    }

    template<class T>
    T get_data() {
        using namespace boost;
        static T value;
        asio::read(serial, asio::buffer(reinterpret_cast<uint8_t*>(&value), sizeof(T)));
        return value;
    }
    
    template <class T, class L>
    std::string readLine(std::function<bool(L)> synchro, [[maybe_unused]] std::function<void(T)> callback) {
        using namespace boost;

        static const auto l_size = sizeof(L);
        static const auto t_size = sizeof(T);

        uint8_t* buffer = static_cast<uint8_t*>(malloc(t_size));

        L* header = reinterpret_cast<L*>(buffer);
        T* data   = reinterpret_cast<T*>(buffer);

        while(!synchro(*header) || true) {
            asio::read(serial, asio::buffer(header, l_size));
        }

        return "";
    }
    
private:
    boost::asio::io_service io;
    boost::asio::serial_port serial;
    
};
