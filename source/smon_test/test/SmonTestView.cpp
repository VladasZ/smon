
#include "Log.hpp"
#include "SmonTestView.hpp"

using namespace ui;

void SmonTestView::_setup() {
    label = new ui::Label();
    add_subview(label);
}

void SmonTestView::_layout() {

    _calculate_absolute_frame();

    label->set_size({ 200, 30 });
    label->set_center(_frame.center());

    _layout_subviews();

}
