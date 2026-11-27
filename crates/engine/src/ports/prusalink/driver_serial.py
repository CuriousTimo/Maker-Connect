import os
import pty
import select
import termios

from prusa.link.serial.serial import Serial

# Each script: open a port ("pty": the slave of a new pseudo terminal; "missing", "file", "dir":
# a path that is not a terminal) at a baudrate, then the ops on it, the other end being the
# pseudo terminal's master: send (bytes to the port), readline, write, receive (what the master
# got, up to 1024 bytes within 1 s), dtr (set), hangup (the master closed), close, state
# (is_open, dtr). Bytes as latin-1 strings. The open's result: the terminal's attributes (the
# cc as numbers), or the exception's class and text; each op's: its value, or the exception's.


def attributes(fd):
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = termios.tcgetattr(fd)
    cc = [c[0] if isinstance(c, bytes) else c for c in cc]
    return [iflag, oflag, cflag, lflag, ispeed, ospeed, cc]


def latin1(data):
    return data.decode("latin-1")


request = json.load(sys.stdin)
directory = request["directory"]  # made by the test, with an empty "file"
regular = os.path.join(directory, "file")
results = []
for script in request["scripts"]:
    master = slave = None
    if script["path"] == "pty":
        master, slave = pty.openpty()
        path = os.ttyname(slave)
    else:
        path = {"missing": os.path.join(directory, "missing"), "file": regular,
                "dir": directory}[script["path"]]
    try:
        port = Serial(path, script["baudrate"], timeout=script["timeout"])
        opened = attributes(port.fd)
    except Exception as error:  # pylint: disable=broad-except
        port = None
        opened = [type(error).__name__, str(error)]
    done = []
    for op in script["ops"] if port else []:
        try:
            if op[0] == "send":
                result = os.write(master, op[1].encode("latin-1"))
            elif op[0] == "readline":
                result = latin1(port.readline())
            elif op[0] == "write":
                result = port.write(op[1].encode("latin-1"))
            elif op[0] == "receive":
                got = b""
                if select.select([master], [], [], 1)[0]:
                    got = os.read(master, 1024)
                result = latin1(got)
            elif op[0] == "dtr":
                port.dtr = op[1]
                result = None
            elif op[0] == "hangup":
                os.close(master)
                result = None
            elif op[0] == "close":
                port.close()
                result = None
            else:
                result = [port.is_open, port.dtr]
        except Exception as error:  # pylint: disable=broad-except
            result = [type(error).__name__, str(error)]
        done.append(result)
    if port:
        port.close()
    for descriptor in (master, slave):
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass
    results.append([opened, done])
print(json.dumps(results))
