
#pragma once

#include <list>
#include <array>
#include <mutex>
#include <thread>

#include "Log.hpp"
#include "DataBuffer.hpp"
#include "DataPacket.hpp"

namespace smon {

    class SerialMonitor {

    public:

        static inline unsigned bytes_received = 0;
        static inline unsigned bytes_sent = 0;

        bool failed_init = false;

        explicit SerialMonitor(const std::string& port, unsigned baud_rate);

        ~SerialMonitor();

        template<class T>
        T& read() {
            static T value;
            value = T{};
            read(value);
            return value;
        }

        template<class T>
        void read(T& value) {
            if constexpr (cu::is_data_v<T>) {
                _read(&value, sizeof(T), T::packet_id);
            }
            else {
                _read(&value, sizeof(T));
            }
        }

        template<class T>
        void write(const T& value) {
            _write(&value, sizeof(T));
        }

        bool has_data();

        void reset();

    private:

        std::mutex mutex;

        void* serial;
        void* io;

        static constexpr auto buffer_size = 2048;
        unsigned unread_count = 0;
        unsigned read_index = 0;
        unsigned write_index = 0;

        std::array<uint8_t, buffer_size> data_buffer;

        std::list<DataBuffer> received_packets;

        void _read(void* buffer, unsigned size, uint16_t id = -1);

        void _write(const void* buffer, unsigned size);

    };

}
