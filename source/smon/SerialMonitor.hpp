
#pragma once

#include <array>
#include <thread>

#include "Log.hpp"

class SerialMonitor {

public:

    static inline unsigned bytes_received = 0;
    static inline unsigned bytes_sent = 0;

    explicit SerialMonitor(const std::string& port, unsigned baud_rate = MBED_SERIAL_BAUD);
    ~SerialMonitor();

    template<class T>
    T& read() {
        static T value;
        value = T { };
        read(value);
        return value;
    }

    template<class T>
    void read(T& value) {
        _read(&value, sizeof(T));
    }

    template<class T>
    void write(const T& value) {
        _write(&value, sizeof(T));
    }

    bool has_data();

    void reset();

    std::string read_string();
    void write_string(const std::string&);

private:

    std::mutex mutex;

    void* serial;
    void* io;

    static constexpr auto buffer_size = 2048;
    unsigned unread_count = 0;
    unsigned read_index = 0;
    unsigned write_index = 0;

    std::array<uint8_t, buffer_size> data_buffer;

    void _read(void* buffer, unsigned size);
    void _write(const void* buffer, unsigned size);

};
