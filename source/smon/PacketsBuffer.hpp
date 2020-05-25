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
            if (_messages.empty()) return;
            _messages_mut.lock();
            Separator;
            Log(std::string() + "Messages: " + std::to_string(_messages.size()));
            for (const auto& error : _messages) {
                Log(error);
            }
            Separator;
            _messages.clear();
            _messages_mut.unlock();
        }

        void force_unlock() {
           // _request_mut.unlock();
        }

    private:

        bool _force_unlock = false;

        std::mutex _request_mut;
        std::mutex _packets_mut;
        std::mutex _messages_mut;
        SerialMonitor& _serial;
        std::list<cu::PacketData> _packets;
        std::vector<cu::BoardMessage> _messages;

    };

}
