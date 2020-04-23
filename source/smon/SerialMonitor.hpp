
#pragma once

namespace smon {

    class SerialMonitor {

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
            _read(&value, sizeof(T));
        }

        template<class T>
        void write(const T& value) {
            _write(&value, sizeof(T));
        }

    private:

        void* serial;
        void* io;

        void _read(void* buffer, unsigned size);
        
        void _write(const void* buffer, unsigned size);

    };

}
