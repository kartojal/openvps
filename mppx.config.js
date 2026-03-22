// OpenVPS — mppx configuration
// Service definition for the MPP payments directory
export default {
  name: 'OpenVPS',
  description: 'AI-agent VPS hosting. Pay with stablecoins, get root SSH access to Ubuntu 24.04 Firecracker microVMs in seconds.',
  url: 'https://openvps.sh',
  skillUrl: 'https://openvps.sh/skill.md',
  methods: ['tempo', 'x402'],
  networks: {
    tempo: {
      chainId: 4217,
      rpcUrl: 'https://rpc.tempo.xyz',
      tokens: [
        { symbol: 'USDC.e', address: '0x20c000000000000000000000b9537d11c60e8b50', decimals: 6 },
        { symbol: 'pathUSD', address: '0x20c0000000000000000000000000000000000000', decimals: 6 },
      ],
    },
    base: {
      chainId: 8453,
      tokens: [
        { symbol: 'USDC', address: '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913', decimals: 6 },
      ],
    },
    celo: {
      chainId: 42220,
      tokens: [
        { symbol: 'USDC', address: '0xcebA9300f2b948710d2653dD7B07f33A8B32118C', decimals: 6 },
      ],
    },
  },
  recipient: '0x8A739f3A6f40194C0128904bC387e63d9C0577A4',
  routes: {
    'POST /v1/provision': {
      description: 'Provision a Firecracker microVM with SSH access',
      pricing: 'dynamic',
      returns: ['vm_id', 'ssh_host', 'ssh_port', 'ssh_private_key'],
    },
    'GET /v1/vms/{id}': {
      description: 'Check VM status',
      pricing: 'free',
    },
    'DELETE /v1/vms/{id}': {
      description: 'Terminate a VM',
      pricing: 'free',
    },
  },
}
