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


PacketsBuffer::PacketsBuffer(SerialMonitor& serial) : _serial(serial) {

}

void PacketsBuffer::start_reading() {

    std::thread([&] {

        EmptyHeader header;
        wipe(header);

        uint8_t byte;

        while (true) {

            _serial.read(byte);

            push_byte(header, byte);

            if (!header.is_valid()) {
                continue;
            }

            PacketData data(header);

            _serial.read(data.data(), header.data_size + sizeof(PacketFooter));

            if (!data.footer()->is_valid()) {
                Log("Invalid footer");
                Log(char_string(PacketFooter::_end_data));
                Log(char_string(data.footer()->value));
                continue;
            }

            if (!data.checksum_is_valid()) {
                Log(std::string() + "Invalid checksum for packet with id: " + std::to_string(data.header.packet_id));
                Log("Packet data:");
                Log(char_string(data.data(), header.data_size));
                continue;
            }

            if (data.header.packet_id == BoardMessage::packet_id) {
                BoardMessage error;
                memcpy(&error, data.data(), sizeof(BoardMessage));
                _messages_mutex.lock();
                _messages.emplace_back(error);
                _messages_mutex.unlock();
                continue;
            }

            _packets_mutex.lock();
            _packets.emplace_back(std::move(data));
            _packets_mutex.unlock();

        }

    }).detach();

}
