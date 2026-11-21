"""Show an authorization URL received on stdin as a local, scannable image."""

import ctypes
import ctypes.util
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GdkPixbuf, GLib, Gtk


class Code(ctypes.Structure):
    _fields_ = [
        ("version", ctypes.c_int),
        ("width", ctypes.c_int),
        ("data", ctypes.POINTER(ctypes.c_ubyte)),
    ]


def modules(text):
    library = ctypes.util.find_library("qrencode")
    if not library:
        raise RuntimeError("libqrencode is unavailable")
    qr = ctypes.CDLL(library)
    qr.QRcode_encodeString8bit.argtypes = [ctypes.c_char_p, ctypes.c_int, ctypes.c_int]
    qr.QRcode_encodeString8bit.restype = ctypes.POINTER(Code)
    qr.QRcode_free.argtypes = [ctypes.POINTER(Code)]
    code = qr.QRcode_encodeString8bit(text, 0, 1)
    if not code:
        raise ValueError("The link does not fit in a QR code")
    try:
        width = code.contents.width
        data = bytes(code.contents.data[:width * width])
    finally:
        qr.QRcode_free(code)
    side = width + 8
    pixels = bytearray(b"\xff" * (side * side * 3))
    for index, value in enumerate(data):
        if value & 1:
            row, column = divmod(index, width)
            start = ((row + 4) * side + column + 4) * 3
            pixels[start:start + 3] = b"\x00\x00\x00"
    return side, bytes(pixels)


def main():
    text = sys.stdin.buffer.readline(8193).rstrip(b"\n")
    if not text or len(text) > 8192 or not Gtk.init_check()[0]:
        return 1
    side, pixels = modules(text)
    pixbuf = GdkPixbuf.Pixbuf.new_from_bytes(
        GLib.Bytes.new(pixels), GdkPixbuf.Colorspace.RGB, False, 8, side, side, side * 3
    )
    display = Gdk.Display.get_default()
    monitor = display.get_primary_monitor() or display.get_monitor(0)
    area = monitor.get_workarea()
    scale = max(1, min(10, (area.width - 64) // side, (area.height - 170) // side))
    size = side * scale
    image = Gtk.Image.new_from_pixbuf(pixbuf.scale_simple(size, size, GdkPixbuf.InterpType.NEAREST))
    window = Gtk.Window(title="Scan Claude sign-in")
    window.set_position(Gtk.WindowPosition.CENTER)
    window.set_resizable(False)
    window.set_border_width(12)
    window.set_keep_above(True)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.add(Gtk.Label(label="Open Camera on your phone and scan to sign in with Claude."))
    box.add(image)
    done = Gtk.Button(label="Return to the private code prompt")
    done.connect("clicked", lambda *_: window.destroy())
    box.add(done)
    window.add(box)
    window.connect("destroy", lambda *_: Gtk.main_quit())
    window.show_all()
    window.present()
    print("ready", flush=True)
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
