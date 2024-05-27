import os
import json
import subprocess

erc20_proc = "../../target/debug/erc20_processor"
if os.name == "nt":
    erc20_proc = erc20_proc.replace("/", "\\") + ".exe"

def endpointf(network, test_endp, no_accounts, do_not_use_contract=False):
    print("Checking endpoint {}".format(test_endp))
    os.system(f"{erc20_proc} generate-key -n {no_accounts} > .env")

    with open("config-payments_template.toml", "r") as f:
        text = f.read().replace("%%RPC_ENDPOINT%%", test_endp)

    with open("config-payments.toml", "w") as f:
        f.write(text)

    comm = [erc20_proc, "balance", "-c", network]
    if do_not_use_contract:
        comm.append("--no-wrapper-contract")
    print("Running command {}".format(" ".join(comm)))
    # Run and get output
    s = subprocess.Popen(comm, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    # Run and get output

    stdout, stderr = s.communicate()

    # load json
    try:
        data = json.loads(stdout)
    except json.JSONDecodeError:
        print("Error: failed to parse JSON")
        print(stdout)
        print(stderr)
        raise
    success_count = 0
    for el in data:
        if data[el]["gas"] != "0":
            raise Exception("Error: gas balance is not 0")
        if data[el]["token"] != "0":
            raise Exception("Error: token balance is not 0")
        success_count += 1
        print(f"{test_endp} - {el} - OK - " + "{} - {}".format(data[el]["gas"], data[el]["token"]))
    if success_count != no_accounts:
        raise Exception("Error: wrong number of accounts")


def check_holesky_endpoints(endpoints):
    for endpoint in endpoints:
        endpointf("holesky", endpoint, 7, do_not_use_contract=False)
    for endpoint in endpoints:
        endpointf("holesky", endpoint, 7, do_not_use_contract=True)


def check_polygon_endpoints(endpoints):
    for endpoint in endpoints:
        endpointf("polygon", endpoint, 7, do_not_use_contract=False)
    for endpoint in endpoints:
        endpointf("polygon", endpoint, 7, do_not_use_contract=True)



if __name__ == '__main__':
    check_polygon_endpoints([
        "https://polygon-pokt.nodies.app",
        "https://polygon-mainnet.public.blastapi.io",
        "https://polygon-pokt.nodies.app",
        "https://1rpc.io/matic",
        "https://polygon-rpc.com",
    ])
    check_holesky_endpoints([
        "https://holesky.drpc.org",
        "https://ethereum-holesky.blockpi.network/v1/rpc/public",
        "https://ethereum-holesky-rpc.publicnode.com"
    ])

