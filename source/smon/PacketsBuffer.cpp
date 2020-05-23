//
//  PacketsBuffer.cpp
//  smon
//
//  Created by Vladas Zakrevskis on 06/05/20.
//  Copyright © 2020 VladasZ. All rights reserved.
//

#include <map>
#include <thread>

#include "Log.hpp"
#include "DataUtils.hpp"
#include "PacketHeader.hpp"
#include "PacketsBuffer.hpp"

using namespace cu;
using namespace smon;

static std::map<PacketsBuffer*, bool> stop;


PacketsBuffer::PacketsBuffer(SerialMonitor& serial) : _serial(serial) {
    stop[this] = false;
    _request_mut.lock();
}

PacketsBuffer::~PacketsBuffer() {
    stop[this] = true;
}

void PacketsBuffer::start_reading() {

    std::thread([&] {

        const bool& _stop = stop[this];

        PacketHeader header;
        wipe(header);

        uint8_t byte;

        while (true) {

            if (_stop) break;

            _serial.read(byte);

            push_byte(header, byte);

            if (!header.is_valid()) {
                continue;
            }

            PacketData data(header);

            _serial.read(data.data(), header.data_size + sizeof(PacketFooter));

            if (!data.footer()->is_valid()) {
                continue;
            }

            if (!data.checksum_is_valid()) {
                Log(std::string() + "Invalid checksum for packet with id: " + std::to_string(data.header.packet_id));
                continue;
            }

            if (data.header.packet_id == BoardMessage::packet_id) {
                BoardMessage error;
                memcpy(&error, data.data(), sizeof(BoardMessage));
                _messages.emplace_back(error);
                _messages_mut.unlock();
                continue;
            }

            _packets_mut.lock();
            _packets.emplace_back(std::move(data));
            _packets_mut.unlock();
            _request_mut.unlock();

        }

        stop.erase(this);

    }).detach();

}
