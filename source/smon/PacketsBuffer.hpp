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
#include "Error.hpp"
#include "PacketData.hpp"
#include "SerialMonitor.hpp"


namespace smon {

    class PacketsBuffer : cu::NonCopyable {

    public:

        explicit PacketsBuffer(SerialMonitor& serial);

        ~PacketsBuffer();

        void start_reading();

        template <class T>
        std::optional<T> get() {
            _mut.lock();

            if (!_errors.empty()) {
                Separator;
                Log("Errors:");
                for (const auto& error : _errors) {
                    Log(error);
                }
                Separator;
                _errors.clear();
            }

            if (_packets.empty()) {
                _mut.unlock();
                return std::nullopt;
            }
            auto& packet = _packets.back();
            T result;
            memcpy(&result, packet.data(), sizeof(T));
            _packets.pop_back();
            _mut.unlock();
            return result;
        }

    private:

        std::mutex _mut;
        SerialMonitor& _serial;
        std::list<cu::PacketData> _packets;
        std::vector<cu::Error> _errors;

    };

}
