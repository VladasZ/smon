
#include <boost/asio.hpp>

#include "Header.hpp"
#include "DataPacket.hpp"
#include "SerialMonitor.hpp"
#include "ExceptionCatch.hpp"

//#define SMON_IGNORE_CONNECTION_ERRORS

using namespace cu;

using namespace std;
using namespace boost;
using namespace boost::asio;

using namespace smon;

#define __SERIAL static_cast<serial_port*>(serial)
#define __IO static_cast<io_service*>(io)

static bool stop = false;
static bool failed_init = false;

SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    stop = false;
    io = new io_service();

    try {
        serial = new serial_port(*__IO, port);
        __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));
    }
    catch(...) {
        Log(what());
#ifdef SMON_IGNORE_CONNECTION_ERRORS
        failed_init = true;
        return;
#else
        Fatal(what());
#endif
    }

    std::thread([&] {

        while(true) {

            if (stop) {
                return;
            }

            static Header header;

            static uint8_t byte;
            asio::read(*__SERIAL, buffer(&byte, 1));
            header.add_byte(byte);

            if (header.is_valid()) {

                DataPacket packet;
                packet.size = header.size;

                for (unsigned i = 0; i < header.size; i++) {
                    asio::read(*__SERIAL, buffer(&byte, 1));
                    packet.data[i]  = byte;
                }

                mutex.lock();
                received_packets.push_back(packet);
                mutex.unlock();
            }

        }

    }).detach();
}

SerialMonitor::~SerialMonitor() {
#ifdef SMON_IGNORE_CONNECTION_ERRORS
    if (failed_init) return;
#endif
    stop = true;
    mutex.lock();
    delete __SERIAL;
    delete __IO;
    mutex.unlock();
}

bool SerialMonitor::has_data() {
    return !received_packets.empty();
}

void SerialMonitor::_read(void* buf, unsigned size) {

#ifdef SMON_IGNORE_CONNECTION_ERRORS
    if (failed_init) return;
#endif

    mutex.lock();

    if (received_packets.size() == 0) {
        mutex.unlock();
        Log("No packets");
        return;
    }

    auto& packet = received_packets.back();

    if (packet.size != size) {
        mutex.unlock();
        Log("Invalid packet");
        return;
    }

    memcpy(buf, &packet.data[0], size);

    received_packets.pop_back();

    mutex.unlock();

    return;
}

void SerialMonitor::_write(const void* buf, unsigned size) {
#ifdef SMON_IGNORE_CONNECTION_ERRORS
    if (failed_init) return;
#endif
    asio::write(*__SERIAL, buffer(buf, size));
    bytes_sent += size;
}
