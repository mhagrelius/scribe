use gtk::prelude::*;
use scribe::ui::ScribeApplication;

fn main() -> gtk::glib::ExitCode {
    gtk::glib::set_application_name("Scribe");
    gtk::glib::set_prgname(Some(scribe::APP_ID));
    ScribeApplication::new().run()
}
