
#pragma once

#include "Log.hpp"

class SerialMonitor {

public:

    explicit SerialMonitor(const std::string& port, unsigned baud_rate = 230400);
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

    std::string read_string();
    void write_string(const std::string&);

private:

    void* _serial;
    void* _io;

    void _read(void* buffer, unsigned size);
    void _write(const void* buffer, unsigned size);

};
