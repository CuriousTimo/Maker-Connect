import re

from prusa.link.serial.instruction import (
    Instruction, MandatoryMatchableInstruction, MatchableInstruction)

# Each case: [kind, message, to_checksum, number for fill_data, matches to capture, force]. The
# result: what each step gives, or the exception's class and text.
request = json.load(sys.stdin)
KINDS = {"plain": Instruction, "matchable": MatchableInstruction,
         "mandatory": MandatoryMatchableInstruction}


def run(kind, message, to_checksum, number, captures, force):
    out = []
    try:
        instruction = KINDS[kind](message, to_checksum=to_checksum)
    except Exception as error:  # pylint: disable=broad-except
        return [{"error": [type(error).__name__, str(error)]}]
    out.append(str(instruction))
    out.append(repr(instruction))
    out.append([instruction.is_sent(), instruction.is_confirmed()])
    try:
        instruction.fill_data(number)
        out.append([list(instruction.data), instruction.number])
    except Exception as error:  # pylint: disable=broad-except
        out.append({"error": [type(error).__name__, str(error)]})
    for text in captures:
        match = re.match(r"(?P<word>\w+)(.*)", text)
        instruction.output_captured(None and 0 or "sender", match)
    if kind != "plain":
        out.append([None if instruction.match(i) is None
                    else instruction.match(i).group(0) for i in range(3)])
        out.append([m.group("word") for m in instruction.get_matches()])
        out.append(instruction.capture_matching.pattern)
    out.append(instruction.wait_for_send(0))
    instruction.sent()
    out.append([instruction.is_sent(), instruction.wait_for_send(0)])
    confirmed = instruction.confirm(force=force)
    out.append([confirmed, instruction.is_confirmed(),
                instruction.wait_for_confirmation(0),
                instruction.time_to_confirm is not None])
    instruction.reset()
    out.append([instruction.is_sent(), instruction.sent_at])
    return out


print(json.dumps({
    "results": [run(*case) for case in request["cases"]],
    "checksums": [Instruction.get_checksum(bytes(data))
                  for data in request["checksums"]],
}))
