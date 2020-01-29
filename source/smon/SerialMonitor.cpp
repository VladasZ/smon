
#include <boost/asio.hpp>

#include "Header.hpp"
#include "DataPacket.hpp"
#include "SerialMonitor.hpp"

using namespace cu;

using namespace std;
using namespace boost;
using namespace boost::asio;

using namespace smon;

#define __SERIAL static_cast<serial_port*>(serial)
#define __IO static_cast<io_service*>(io)

static bool stop = false;

SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    stop = false;
    io = new io_service();
    serial = new serial_port(*__IO, port);
    __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));

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

                Logvar(header.size);

                for (unsigned i = 0; i < header.size; i++) {
                    asio::read(*__SERIAL, buffer(&byte, 1));
                    packet.data[i]  = byte;
                }

                mutex.lock();
                received_packets.push_back(packet);
                Log("Packet ready");
                mutex.unlock();
            }

//            mutex.lock();
//            data_buffer[write_index++] = byte;
//            unread_count++;
//            bytes_received++;
//            if (write_index == data_buffer.size()) {
//                write_index = 0;
//            }
//            mutex.unlock();
        }

    }).detach();
}

SerialMonitor::~SerialMonitor() {
    stop = true;
    mutex.lock();
    delete __SERIAL;
    delete __IO;
    mutex.unlock();
}

bool SerialMonitor::has_data() {
//    bool result = false;
//    mutex.lock();
//    result = unread_count > 0;
//    mutex.unlock();
    return true;
}

void SerialMonitor::_read(void* buf, unsigned size) {
    mutex.lock();

    if (received_packets.size() == 0) {
        mutex.unlock();
        Log("No packets");
        return;
    }

    auto& packet = received_packets.back();

    Logvar(packet.size);
    Logvar(size);

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
    asio::write(*__SERIAL, buffer(buf, size));
    bytes_sent += size;
}
