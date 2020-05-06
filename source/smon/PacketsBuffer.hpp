//
//  PacketsBuffer.hpp
//  smon
//
//  Created by Vladas Zakrevskis on 06/05/20.
//  Copyright © 2020 VladasZ. All rights reserved.
//

#pragma once


#include "SerialMonitor.hpp"
#include "CircularBuffer.hpp"


namespace smon {

    class PacketsBuffer {

    public:

        PacketsBuffer(SerialMonitor& serial);

        ~PacketsBuffer();

        void start_reading();


    private:

        SerialMonitor& _serial;
        cu::CircularBuffer<1024> _buffer;

    };

}