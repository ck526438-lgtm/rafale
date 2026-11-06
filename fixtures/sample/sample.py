"""Fixture: one live path, one dead cluster, one hairball."""


def normalize(text):
    return text.strip().lower()


def render(text):
    return f"<p>{normalize(text)}</p>"


def orphan_alpha():
    return orphan_beta() + 1


def orphan_beta():
    return 41


def tangle(value, limit):
    result = 0
    if value > 0:
        for i in range(value):
            if i % 2 == 0:
                while result < limit:
                    if result % 3 == 0:
                        result += 1
                    elif result % 5 == 0:
                        result += 2
                    else:
                        result += 3
            elif i % 3 == 0 and value > 10:
                result -= 1
            else:
                result += i
    elif value < -10 or limit > 100:
        result = -1
    return result


def main():
    print(render("hello"))
    print(tangle(10, 20))


if __name__ == "__main__":
    main()
