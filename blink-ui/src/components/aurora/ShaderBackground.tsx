import { useRef, useMemo } from 'react'
import { Canvas, useFrame } from '@react-three/fiber'
import * as THREE from 'three'

const vertexShader = `
varying vec2 vUv;
void main() {
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`

const fragmentShader = `
uniform float u_time;
uniform vec3 u_color1;
uniform vec3 u_color2;
uniform vec3 u_color3;
varying vec2 vUv;

// Simplex 2D noise
vec3 permute(vec3 x) { return mod(((x*34.0)+1.0)*x, 289.0); }

float snoise(vec2 v){
  const vec4 C = vec4(0.211324865405187, 0.366025403784439,
           -0.577350269189626, 0.024390243902439);
  vec2 i  = floor(v + dot(v, C.yy) );
  vec2 x0 = v -   i + dot(i, C.xx);
  vec2 i1;
  i1 = (x0.x > x0.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
  vec4 x12 = x0.xyxy + C.xxzz;
  x12.xy -= i1;
  i = mod(i, 289.0);
  vec3 p = permute( permute( i.y + vec3(0.0, i1.y, 1.0 ))
  + i.x + vec3(0.0, i1.x, 1.0 ));
  vec3 m = max(0.5 - vec3(dot(x0,x0), dot(x12.xy,x12.xy),
    dot(x12.zw,x12.zw)), 0.0);
  m = m*m ;
  m = m*m ;
  vec3 x = 2.0 * fract(p * C.www) - 1.0;
  vec3 h = abs(x) - 0.5;
  vec3 ox = floor(x + 0.5);
  vec3 a0 = x - ox;
  m *= 1.79284291400159 - 0.85373472095314 * ( a0*a0 + h*h );
  vec3 g;
  g.x  = a0.x  * x0.x  + h.x  * x0.y;
  g.yz = a0.yz * x12.xz + h.yz * x12.yw;
  return 130.0 * dot(m, g);
}

void main() {
  vec2 uv = vUv;
  
  // Distortion based on noise
  float noise = snoise(uv * 2.0 + u_time * 0.1);
  float noise2 = snoise(uv * 3.0 - u_time * 0.15);
  
  // Color mixing
  vec3 color = mix(u_color1, u_color2, smoothstep(-1.0, 1.0, noise + uv.x));
  color = mix(color, u_color3, smoothstep(-1.0, 1.0, noise2 + uv.y));
  
  // Vignette
  float vignette = length(uv - 0.5);
  color *= smoothstep(0.8, 0.2, vignette);
  
  // Opacity fade
  gl_FragColor = vec4(color, 0.4 - vignette * 0.3);
}
`

function AuroraPlane() {
  const mesh = useRef<THREE.Mesh>(null)
  
  const uniforms = useMemo(
    () => ({
      u_time: { value: 0 },
      u_color1: { value: new THREE.Color('#00f0ff') }, // cyan
      u_color2: { value: new THREE.Color('#8a2be2') }, // violet
      u_color3: { value: new THREE.Color('#101030') }  // void
    }),
    []
  )

  useFrame((state) => {
    if (mesh.current) {
      const material = mesh.current.material as THREE.ShaderMaterial
      material.uniforms.u_time.value = state.clock.elapsedTime
    }
  })

  return (
    <mesh ref={mesh}>
      <planeGeometry args={[2, 2, 32, 32]} />
      <shaderMaterial
        vertexShader={vertexShader}
        fragmentShader={fragmentShader}
        uniforms={uniforms}
        transparent={true}
        depthWrite={false}
      />
    </mesh>
  )
}

export default function ShaderBackground() {
  return (
    <div className="absolute inset-0 z-0 pointer-events-none overflow-hidden bg-[color:var(--color-void)]">
      <Canvas orthographic camera={{ position: [0, 0, 1], zoom: 1 }}>
        <AuroraPlane />
      </Canvas>
    </div>
  )
}