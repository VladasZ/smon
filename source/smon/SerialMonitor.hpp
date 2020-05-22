
#pragma once

#include <mutex>

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

        std::mutex mutex;

    public:

        void read(void* buffer, unsigned size);
        void write(const void* buffer, unsigned size);

        void lock();
        void unlock();

        void sync(std::function<void(SerialMonitor&)> action) {
            lock();
            action(*this);
            unlock();
        }

    };

}
