
#include <boost/asio.hpp>

#include "Log.hpp"
#include "SerialMonitor.hpp"
#include "ExceptionCatch.hpp"

using namespace cu;

using namespace std;
using namespace boost;
using namespace boost::asio;

using namespace smon;


#define __SERIAL static_cast<serial_port*>(serial)
#define __IO static_cast<io_service*>(io)


SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    io = new io_service();

    try {
        serial = new serial_port(*__IO, port);
        __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));
    }
    catch(...) {
        Fatal(what());
    }
}

SerialMonitor::~SerialMonitor() {
    delete __SERIAL;
    delete __IO;
}

void SerialMonitor::_read(void* buf, unsigned size) {
    asio::read(*__SERIAL, buffer(buf, size));
}

void SerialMonitor::_write(const void* buf, unsigned size) {
    asio::write(*__SERIAL, buffer(buf, size));
}
