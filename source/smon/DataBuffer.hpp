//
//  DataPacket.hpp
//  smon
//
//  Created by Vladas Zakrevskis on 29/01/20.
//  Copyright © 2020 VladasZ. All rights reserved.
//

#pragma once

#include <array>

namespace smon {

    class DataBuffer {

    public:

        unsigned size;
        uint16_t packet_id;

        std::array<uint8_t, 256> data;

    };

}