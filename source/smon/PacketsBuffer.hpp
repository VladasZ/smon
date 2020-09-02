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

        void start_reading();

        template <class T>
        T get() {

            if (_packets.empty()) {
                Log << "No packets";
                return T { };
            }

            auto& packet = _packets.back();
            T result;
            memcpy(&result, packet.data(), sizeof(T));
            _packets_mutex.lock();
            _packets.pop_back();
            _packets_mutex.unlock();

            return result;
        }

        void check_messages() {
            for (const auto& message : _messages) {
                std::cout << "STM32: ";
                CleanLog(message);
            }
            _messages_mutex.lock();
            _messages.clear();
            _messages_mutex.unlock();
        }

    private:

        SerialMonitor& _serial;
        std::list<cu::PacketData> _packets;
        std::vector<cu::BoardMessage> _messages;

        std::mutex _packets_mutex;
        std::mutex _messages_mutex;

    };

}
