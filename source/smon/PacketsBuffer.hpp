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
        T get() {

            _request_mut.lock();
//
//            if (_force_unlock) {
//                _force_unlock = false;
//                _request_mut.lock();
//                return { };
//            }

            _packets_mut.lock();

            auto& packet = _packets.back();
            T result;
            memcpy(&result, packet.data(), sizeof(T));
            _packets.pop_back();

            _packets_mut.unlock();

            return result;
        }

        void check_errors() {
            if (_errors.empty()) return;
            _errors_mut.lock();
            Separator;
            Log(std::string() + "Errors: " + std::to_string(_errors.size()));
            for (const auto& error : _errors) {
                Log(error);
            }
            Separator;
            _errors.clear();
            _errors_mut.unlock();
        }

        void force_unlock() {
           // _request_mut.unlock();
        }

    private:

        bool _force_unlock = false;

        std::mutex _request_mut;
        std::mutex _packets_mut;
        std::mutex _errors_mut;
        SerialMonitor& _serial;
        std::list<cu::PacketData> _packets;
        std::vector<cu::Error> _errors;

    };

}
