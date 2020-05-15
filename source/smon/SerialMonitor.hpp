
#pragma once

#include "NonCopyable.hpp"


namespace smon {

    class SerialMonitor : cu::NonCopyable {

    public:

        explicit SerialMonitor(const std::string& port, unsigned baud_rate);

        ~SerialMonitor();

        template<class T>
        T read() {
            T value;
            read(value);
            return value;
        }

        template<class T>
        void read(T& value) {
            read(&value, sizeof(T));
        }

        template<class T>
        void write(const T& value) {
            write(&value, sizeof(T));
        }

    private:

        void* serial;
        void* io;

    public:

        void read(void* buffer, unsigned size);
        void write(const void* buffer, unsigned size);

    };

}
