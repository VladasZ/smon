//
//  PacketsBuffer.cpp
//  smon
//
//  Created by Vladas Zakrevskis on 06/05/20.
//  Copyright © 2020 VladasZ. All rights reserved.
//

#include <map>
#include <thread>

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
        memset(&header, 0, sizeof(header));

        while (true) {

            if (_stop) break;



        }

        stop.erase(this);

    }).detach();

}
