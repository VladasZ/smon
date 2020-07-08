//
//  PacketsBuffer.hpp
//  smon
//
//  Created by Vladas Zakrevskis on 06/05/20.
//  Copyright © 2020 VladasZ. All rights reserved.
//

#pragma once

#include <list>
#include <mutex>
#include <optional>

#include "Log.hpp"
#include "BoardMessage.hpp"
#include "PacketData.hpp"
#include "SerialMonitor.hpp"


namespace smon {

    class PacketsBuffer : cu::NonCopyable {

    public:

        explicit PacketsBuffer(SerialMonitor& serial);

        ~PacketsBuffer();

        void start_reading();

        template <class T>
        T get() {
            _has_packets_mut.lock();
            _packets_mut.lock();

            if (_packets.empty()) {
                Log("No packets");
                _packets_mut.unlock();
                return { };
            }

            auto& packet = _packets.back();
            T result;
            memcpy(&result, packet.data(), sizeof(T));
            _packets.pop_back();

            _packets_mut.unlock();

            return result;
        }

        void check_messages() {
            _has_messages_mut.lock();
            _messages_mut.lock();
            for (const auto& message : _messages) {
                std::cout << "STM32: ";
                CleanLog(message);
            }
            _messages.clear();
            _messages_mut.unlock();
        }

        void force_unlock() {
            _messages.clear();
            _packets.clear();
            _has_packets_mut.unlock();
            _packets_mut.unlock();
        }

    private:

        std::mutex _packets_mut;
        std::mutex _has_packets_mut;

        std::mutex _messages_mut;
        std::mutex _has_messages_mut;

        SerialMonitor& _serial;
        std::list<cu::PacketData> _packets;
        std::vector<cu::BoardMessage> _messages;

    };

}
