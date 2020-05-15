
#include <boost/asio.hpp>

#include "SerialMonitor.hpp"


using namespace std;
using namespace boost;
using namespace boost::asio;

using namespace smon;


#define __SERIAL static_cast<serial_port*>(serial)
#define __IO static_cast<io_service*>(io)


SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    io = new io_service();
    serial = new serial_port(*__IO, port);
    __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));
}

SerialMonitor::~SerialMonitor() {
    delete __SERIAL;
    delete __IO;
}

void SerialMonitor::read(void* buf, unsigned size) {
    asio::read(*__SERIAL, buffer(buf, size));
}

void SerialMonitor::write(const void* buf, unsigned size) {
    asio::write(*__SERIAL, buffer(buf, size));
}

void SerialMonitor::lock() {
    mutex.lock();
}

void SerialMonitor::unlock() {
    mutex.unlock();
}
