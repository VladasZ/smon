
#include <boost/asio.hpp>

#include "SerialMonitor.hpp"

using namespace std;
using namespace boost;
using namespace boost::asio;

#define __SERIAL static_cast<serial_port*>(_serial)
#define __IO static_cast<io_service*>(_io)

SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    _io = new io_service();
    _serial = new serial_port(*__IO, port);
    __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));
}

SerialMonitor::~SerialMonitor() {
    delete __SERIAL;
    delete __IO;
}

std::string SerialMonitor::read_string() {
    static char buffer[1024];

    for (int i = 0; i < 1024; i++) {
        auto letter = read<char>();
        buffer[i] = letter;
        if (letter == '\n') {
            break;
        }
    }

    return buffer;
}

void SerialMonitor::write_string(const string& str) {
    _write(str.c_str(), str.size());
}

void SerialMonitor::_read(void* buf, unsigned size) {
    asio::read(*__SERIAL, buffer(buf, size));
}

void SerialMonitor::_write(const void* buf, unsigned size) {
    asio::write(*__SERIAL, buffer(buf, size));
}
