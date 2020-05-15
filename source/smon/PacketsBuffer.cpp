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

            _mut.lock();
            _packets.emplace_back(std::move(data));
            _mut.unlock();

        }

        stop.erase(this);

    }).detach();

}
