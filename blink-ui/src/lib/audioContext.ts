// Procedural Audio Engine for Terminal Omega
class AudioEngine {
  private ctx: AudioContext | null = null;
  private masterGain: GainNode | null = null;

  init() {
    if (this.ctx) return;
    this.ctx = new (window.AudioContext || (window as any).webkitAudioContext)();
    this.masterGain = this.ctx.createGain();
    this.masterGain.gain.value = 0.2; // Keep it subtle
    this.masterGain.connect(this.ctx.destination);
  }

  playTrade(side: 'buy' | 'sell', volume: number = 0.5) {
    if (!this.ctx || !this.masterGain) return;
    
    // Resume context if suspended (browser policy)
    if (this.ctx.state === 'suspended') this.ctx.resume();

    const osc = this.ctx.createOscillator();
    const gain = this.ctx.createGain();
    
    // Frequency based on side
    osc.frequency.setValueAtTime(side === 'buy' ? 880 : 320, this.ctx.currentTime); // High for buy, Low for sell
    
    // Envelope
    gain.gain.setValueAtTime(0, this.ctx.currentTime);
    gain.gain.linearRampToValueAtTime(volume, this.ctx.currentTime + 0.01);
    gain.gain.exponentialRampToValueAtTime(0.001, this.ctx.currentTime + 0.3);
    
    osc.connect(gain);
    gain.connect(this.masterGain);
    
    osc.start(this.ctx.currentTime);
    osc.stop(this.ctx.currentTime + 0.3);
  }

  playAlert() {
    if (!this.ctx || !this.masterGain) return;
    if (this.ctx.state === 'suspended') this.ctx.resume();

    const osc = this.ctx.createOscillator();
    const gain = this.ctx.createGain();
    
    osc.type = 'sawtooth';
    osc.frequency.setValueAtTime(120, this.ctx.currentTime);
    osc.frequency.exponentialRampToValueAtTime(60, this.ctx.currentTime + 0.5);
    
    gain.gain.setValueAtTime(0, this.ctx.currentTime);
    gain.gain.linearRampToValueAtTime(0.8, this.ctx.currentTime + 0.05);
    gain.gain.exponentialRampToValueAtTime(0.001, this.ctx.currentTime + 0.5);
    
    osc.connect(gain);
    gain.connect(this.masterGain);
    
    osc.start(this.ctx.currentTime);
    osc.stop(this.ctx.currentTime + 0.5);
  }
}

export const audio = new AudioEngine();