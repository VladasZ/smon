
#pragma once

#include "View.hpp"
#include "Label.hpp"

class SmonTestView : public ui::View {

public:

    ui::Label* label;

protected:

    void _setup() override;
    void _layout() override;

};
