
#include <boost/asio.hpp>

#include "SerialMonitor.hpp"

using namespace std;
using namespace boost;
using namespace boost::asio;

#define __SERIAL static_cast<serial_port*>(serial)
#define __IO static_cast<io_service*>(io)

static bool stop = false;

SerialMonitor::SerialMonitor(const string& port, unsigned baud_rate) {
    stop = false;
    io = new io_service();
    serial = new serial_port(*__IO, port);
    __SERIAL->set_option(serial_port_base::baud_rate(baud_rate));

//    std::thread([&] {
//
//        while(true) {
//
//            if (stop) {
//                return;
//            }
//
//            static uint8_t byte;
//            asio::read(*__SERIAL, buffer(&byte, 1));
//            Logvar(static_cast<int>(byte));
//
//            mutex.lock();
//            data_buffer[write_index++] = byte;
//            unread_count++;
//            bytes_received++;
//            if (write_index == data_buffer.size()) {
//                write_index = 0;
//            }
//            mutex.unlock();
//        }
//
//    }).detach();
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

//
//    mutex.lock();
//    if (unread_count == 0) {
//        mutex.unlock();
//        return;
//    }
//    // Logvar(size);
//    // Logvar(unread_count);
//    if (unread_count < size) {
//        Log("Not enough data in buffer");
//        memset(buf, 0, size);
//        mutex.unlock();
//        return;
//    }
//    if (read_index + size < buffer_size) {
//        memcpy(buf, &data_buffer[read_index], size);
//        unread_count -= size;
//        read_index += size;
//        if (read_index == data_buffer.size()) {
//            read_index = 0;
//        }
//    }
//    else {
//        Log("IMPLEMENT");
//        read_index = 0;
//        write_index = 0;
//        unread_count = 0;
//        memset(buf, 0, size);
//    }
//    mutex.unlock();
}

void SerialMonitor::_write(const void* buf, unsigned size) {
    asio::write(*__SERIAL, buffer(buf, size));
    bytes_sent += size;
}
