use gtk::prelude::*;
use mynah::ui::MynahApplication;

fn main() -> gtk::glib::ExitCode {
    gtk::glib::set_application_name("Mynah");
    gtk::glib::set_prgname(Some(mynah::APP_ID));
    MynahApplication::new().run()
}
