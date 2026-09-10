// Installation tabs functionality
document.addEventListener('DOMContentLoaded', () => {
    const tabs = document.querySelectorAll('.install-tab');
    const panes = document.querySelectorAll('.install-pane');

    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const target = tab.dataset.tab;

            tabs.forEach(t => t.classList.remove('active'));
            panes.forEach(p => p.classList.remove('active'));

            tab.classList.add('active');
            document.getElementById(target).classList.add('active');
        });
    });

    // Copy to clipboard functionality
    document.querySelectorAll('.copy-btn').forEach(btn => {
        btn.addEventListener('click', async () => {
            const text = btn.dataset.copy;
            try {
                await navigator.clipboard.writeText(text);
            } catch (err) {
                // Fallback for older browsers
                const textarea = document.createElement('textarea');
                textarea.value = text;
                textarea.style.position = 'fixed';
                textarea.style.opacity = '0';
                document.body.appendChild(textarea);
                textarea.select();
                document.execCommand('copy');
                document.body.removeChild(textarea);
            }
            btn.classList.add('copied');
            setTimeout(() => btn.classList.remove('copied'), 2000);
        });
    });

    initTerminalDemo();
});

// Terminal demo animation. Every prompt and option below is the CLI's real
// text (see src/main.rs and src/commands/**), so the demo never drifts from
// what users actually see.
function initTerminalDemo() {
    const terminal = document.getElementById('terminal-demo');
    if (!terminal) return;

    const hint = { type: 'hint', text: '  [↑↓ to move, enter to select, type to filter]', delay: 200 };

    const sequences = [
        {
            title: 'Interactive Mode',
            lines: [
                { type: 'command', text: '$ omni', delay: 500 },
                { type: 'empty', delay: 300 },
                { type: 'prompt', text: '◆ What are you up to?', delay: 100 },
                { type: 'option-selected', text: '  ● account     -   Inspect derived foreign accounts (addresses, balances)', delay: 80 },
                { type: 'option', text: '  ○ transaction -   Construct, sign with MPC, and broadcast transactions on other chains', delay: 60 },
                { type: 'option', text: '  ○ proposal    -   List, review (verify!), and vote on DAO chain-signature proposals', delay: 60 },
                { type: 'option', text: '  ○ config      -   Manage the omni chain registry (add chains, sync defaults, reset)', delay: 60 },
                { type: 'option', text: '  ○ self-update -   Update omni to the latest GitHub release', delay: 60 },
                hint,
            ]
        },
        {
            title: 'Derived accounts',
            lines: [
                { type: 'command', text: '$ omni account show bridge-dao.sputnik-dao.testnet omni-1 network-config testnet', delay: 500 },
                { type: 'empty', delay: 300 },
                { type: 'output', text: 'Derived addresses for bridge-dao.sputnik-dao.testnet / "omni-1" (NEAR testnet):', delay: 200 },
                { type: 'output', text: '+--------+--------------------------------------------------------------------+-----------------------------------------+', delay: 60 },
                { type: 'output', text: '| Family | Derived address                                                    | Chains                                  |', delay: 60 },
                { type: 'output', text: '+--------+--------------------------------------------------------------------+-----------------------------------------+', delay: 60 },
                { type: 'output', text: '| aptos  | 0x3b38c7fe9c97541d67214cf2af77a961b96368acae724056d3c7eeeefff34f79 | aptos                                   |', delay: 80 },
                { type: 'output', text: '| evm    | 0x5c30aFDa26a4DEFDc8b365Bd84BcABb8b7526Ba8                         | abs, arb, base, bnb, eth, hyperevm, pol |', delay: 80 },
                { type: 'output', text: '| sui    | 0xf9272534931b1066e370332036c30b397610182e46e1f3f7af0e9e5ff9828699 | sui                                     |', delay: 80 },
                { type: 'output', text: '| svm    | 8nz16pYg9a6pnLq7anqiWCBfGhSqmJQvzSBWJJ1HmPgg                       | fogo, solana                            |', delay: 80 },
                { type: 'output', text: '| ton    | 0QAV98UtOI5NbMm6-sodLeR-HhM2-HWIzFqP_-6FoTF-Z1en                   | ton                                     |', delay: 80 },
                { type: 'output', text: '| utxo   | tb1q8p925070fde2dtnc9cnzlk4r6s3p3z9v33fx9u                         | btc                                     |', delay: 80 },
                { type: 'output', text: '+--------+--------------------------------------------------------------------+-----------------------------------------+', delay: 60 },
            ]
        },
        {
            title: 'Construct',
            lines: [
                { type: 'command', text: '$ omni transaction construct', delay: 500 },
                { type: 'empty', delay: 300 },
                { type: 'prompt', text: '◆ Select the destination chain family:', delay: 100 },
                { type: 'option-selected', text: '  ● evm         -   EVM chains (Ethereum, Base, Arbitrum, ...)', delay: 80 },
                { type: 'option', text: '  ○ svm         -   SVM chains (Solana, Fogo, ...)', delay: 60 },
                { type: 'option', text: '  ○ utxo        -   UTXO chains (Bitcoin; P2WPKH, one MPC signature per input)', delay: 60 },
                { type: 'option', text: '  ○ aptos       -   Aptos', delay: 60 },
                { type: 'option', text: '  ○ sui         -   Sui', delay: 60 },
                { type: 'option', text: '  ○ ton         -   TON (v5r1 wallet, auto-deployed on first use)', delay: 60 },
                hint,
                { type: 'empty', delay: 700 },
                { type: 'prompt', text: '◆ Which evm chain?', delay: 100 },
                { type: 'option-selected', text: '  ● base', delay: 80 },
                { type: 'option', text: '  ○ eth', delay: 60 },
                { type: 'option', text: '  ○ arb', delay: 60 },
                { type: 'option', text: '  ○ bnb', delay: 60 },
                { type: 'empty', delay: 700 },
                { type: 'prompt', text: '◆ Select the action:', delay: 100 },
                { type: 'option-selected', text: '  ● transfer       -   Transfer the native token (ETH, ...)', delay: 80 },
                { type: 'option', text: '  ○ contract-call  -   Call a contract function (typed signature or raw calldata)', delay: 60 },
                { type: 'option', text: '  ○ raw            -   Fully custom transaction: recipient, value, and raw calldata', delay: 60 },
                hint,
            ]
        },
        {
            title: 'Review a proposal',
            lines: [
                { type: 'command', text: '$ omni proposal review bridge-dao.sputnik-dao.testnet 42 network-config testnet', delay: 500 },
                { type: 'empty', delay: 300 },
                { type: 'output', text: 'Proposal #42 on bridge-dao.sputnik-dao.testnet [InProgress] (proposed by alice.testnet)', delay: 200 },
                { type: 'output', text: 'intent:          Pause Base locker during incident #42', delay: 80 },
                { type: 'output', text: 'chain:           base (evm, NEAR testnet)', delay: 80 },
                { type: 'output', text: 'acting account:  0x5c30aFDa26a4DEFDc8b365Bd84BcABb8b7526Ba8 (bridge-dao.sputnik-dao.testnet / "omni-1")', delay: 80 },
                { type: 'empty', delay: 400 },
                { type: 'ok', text: '[OK] receiver is the MPC signer contract (v1.signer-prod.testnet)', delay: 150 },
                { type: 'ok', text: '[OK] 1 action(s), all plain `sign` calls', delay: 150 },
                { type: 'ok', text: '[OK] derivation path matches the envelope ("omni-1")', delay: 150 },
                { type: 'ok', text: "[OK] key domain matches the 'evm' family (0)", delay: 150 },
                { type: 'ok', text: '[OK] 1 signing payload(s) match the envelope byte-for-byte', delay: 150 },
                { type: 'empty', delay: 300 },
                { type: 'ok', text: 'VERIFIED: the MPC would sign exactly the transaction shown above.', delay: 200 },
                { type: 'output', text: 'Vote with:', delay: 100 },
                { type: 'warn', text: '  omni proposal vote bridge-dao.sputnik-dao.testnet 42 approve <your-account> ...', delay: 80 },
            ]
        }
    ];

    let currentSequence = 0;

    async function runSequence(sequence) {
        terminal.innerHTML = '';

        for (const line of sequence.lines) {
            await delay(line.delay);
            appendLine(line);
        }

        // Wait before showing next sequence
        await delay(3500);
    }

    function appendLine(line) {
        const div = document.createElement('div');
        div.className = 'terminal-line';

        switch (line.type) {
            case 'command':
                div.innerHTML = `<span class="terminal-prompt">$</span> <span class="terminal-command">${escapeHtml(line.text.slice(2))}</span><span class="terminal-cursor"></span>`;
                break;
            case 'empty':
                div.innerHTML = '&nbsp;';
                break;
            case 'prompt':
                div.innerHTML = `<span class="terminal-highlight">◆</span> <span class="terminal-output">${escapeHtml(line.text.slice(2))}</span>`;
                break;
            case 'option-selected':
                div.innerHTML = `<span class="terminal-dim">│</span> <span class="terminal-highlight">●</span> <span class="terminal-output">${escapeHtml(line.text.slice(4))}</span>`;
                break;
            case 'option':
                div.innerHTML = `<span class="terminal-dim">│</span> <span class="terminal-dim">○</span> <span class="terminal-dim">${escapeHtml(line.text.slice(4))}</span>`;
                break;
            case 'hint':
                div.innerHTML = `<span class="terminal-dim">│</span> <span class="terminal-warn">${escapeHtml(line.text.slice(2))}</span>`;
                break;
            case 'output':
                div.innerHTML = `<span class="terminal-output">${escapeHtml(line.text)}</span>`;
                break;
            case 'ok':
                div.innerHTML = `<span class="terminal-ok">${escapeHtml(line.text)}</span>`;
                break;
            case 'warn':
                div.innerHTML = `<span class="terminal-warn">${escapeHtml(line.text)}</span>`;
                break;
            default:
                div.textContent = line.text;
        }

        terminal.appendChild(div);

        // Remove cursor from previous command line
        const cursors = terminal.querySelectorAll('.terminal-cursor');
        if (cursors.length > 1) {
            cursors[0].remove();
        }
    }

    function delay(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    function escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    async function runLoop() {
        while (true) {
            await runSequence(sequences[currentSequence]);
            currentSequence = (currentSequence + 1) % sequences.length;
        }
    }

    runLoop();
}

// Smooth scroll for anchor links
document.querySelectorAll('a[href^="#"]').forEach(anchor => {
    anchor.addEventListener('click', function(e) {
        e.preventDefault();
        const target = document.querySelector(this.getAttribute('href'));
        if (target) {
            target.scrollIntoView({ behavior: 'smooth', block: 'start' });
        }
    });
});

// Hover effect on mini-terminals: the hovered option becomes the selection
document.querySelectorAll('.term-option').forEach(option => {
    option.addEventListener('mouseenter', function() {
        const parent = this.closest('.term-options');
        if (!parent) return;

        parent.querySelectorAll('.term-option').forEach(opt => {
            opt.classList.remove('selected');
            const radio = opt.querySelector('.radio');
            if (radio) radio.classList.remove('filled');
        });

        this.classList.add('selected');
        const radio = this.querySelector('.radio');
        if (radio) radio.classList.add('filled');
    });
});

// Reset mini-terminal to its first option on mouse leave
document.querySelectorAll('.mini-terminal').forEach(terminal => {
    terminal.addEventListener('mouseleave', function() {
        this.querySelectorAll('.term-option').forEach((opt, index) => {
            const radio = opt.querySelector('.radio');
            opt.classList.toggle('selected', index === 0);
            if (radio) radio.classList.toggle('filled', index === 0);
        });
    });
});
