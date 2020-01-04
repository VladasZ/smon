
#include "Screen.hpp"
#include "SmonTestView.hpp"

void smon_ui_test() {
    auto screen = new te::Screen({ 800, 680 });
    auto view = new SmonTestView();
    screen->clear_color = gm::Color::gray;
    screen->set_view(view);

    view->label->set_text("Hello");
    view->label->resize_to_fit_text();

    screen->start_main_loop();
}