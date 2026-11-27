# Muse agent setup

Muse is Meta's consumer agent that runs inside the Muse VM. Everett registers
it as an external inbox agent so your coding agent can send it messages.

Muse Code is the `muse` terminal CLI. Everett discovers its local sessions in
`~/.local/share/muse`.

Give your coding agent this setup request.

> Connect this computer to my Muse VM using the steps below. Run the sender
> commands here and ask the Muse agent to run the VM commands in its terminal.
> Show me the device approval URL if sign-in is needed. Send one status check
> and report the reply you receive from Muse.

## 1. Register on the sending computer

Run this on the computer that will send messages to Muse.

```bash
everett external add --id muse --harness muse-agent --title "Muse VM agent"
```

The sender will use `muse` as its local name for the remote inbox after step 4.

## 2. Register the receiving inbox inside the Muse VM

Ask the Muse agent to install Everett in the VM and start sign-in there.

```bash
# inside the Muse VM
everett login --provider cloudflare
```

Open the approval URL and approve the same Everett account used on the sending
computer. If the sender also needs sign-in, run the same login command there.
See [account setup](auth.md).

Have the Muse agent create an unbound inbox in the same Everett profile that
will run the VM's relay listener. The relay checks for the exact `muse-agent/muse`
registration before it imports a message.

```bash
# inside the Muse VM
everett external add --id muse --harness muse-agent
```

## 3. Start the relay on both devices

```bash
# on both devices
everett relay register
```

On the sender, run `everett relay devices --names` and identify the VM's unique
device name. Keep `everett relay` running on both devices, in separate terminal
sessions from the remaining commands. See [Cloudflare native relay](cloudflare-native.md)
for device selection and same-account pairing.

## 4. Replace the sender registration with a relay binding

On the sending computer, replace `PEER` with the VM's unique device name. If
the name is ambiguous, use `id:DEVICE_ID` in its place. Keep `muse-agent/muse`
to match the VM registration from step 2.

```bash
everett external add --id muse --harness muse-agent --title "Muse VM agent" \
  --replace --device 'PEER/muse-agent/muse'
```

The sender now queues messages for `muse` on the encrypted relay for the VM to
pick up.

## 5. Verify pickup and reply

Have the coding agent send a status check from the sending computer.

```bash
everett send "status check" --to muse --mode inbox --json
```

In the VM, have the Muse agent read its inbox and reply using the message's
local `id` from the inbox output.

```bash
everett inbox --session muse --json
EVERETT_SESSION_ID=muse EVERETT_HARNESS_NAME=muse-agent \
  everett reply LOCAL_MESSAGE_ID "Muse inbox ready"
```

On the sender, use `everett peer status 'PEER' RELAY_MESSAGE_ID` with the
`message_id` from the send result. `queued` means the relay retained the message.
`claimed` means the Muse inbox was read. `replied` means Everett observed a reply.
The Muse agent must read its inbox to pick up work because it has no native wake.
If the VM's relay reports `recipient session not found`, check its Everett
profile and the exact `muse-agent/muse` binding.
