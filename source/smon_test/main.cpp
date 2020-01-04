
#include "Screen.hpp"
#include "SmonTestView.hpp"

int main() {

    auto screen = new te::Screen({ 800, 680 });
    auto view = new SmonTestView();
    screen->set_view(view);

    view->label->set_text("Hello");
    view->label->resize_to_fit_text();

    screen->start_main_loop();

	return 0;
}
