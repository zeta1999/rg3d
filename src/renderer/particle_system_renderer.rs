use crate::{
    renderer::{
        GlState,
        geometry_buffer::{
            GeometryBuffer,
            GeometryBufferKind,
            AttributeDefinition,
            AttributeKind,
            ElementKind,
        },
        gl,
        gpu_program::{GpuProgram, UniformLocation},
        gbuffer::GBuffer,
        error::RendererError,
        gpu_texture::GpuTexture,
        RenderPassStatistics,
    },
    scene::{
        node::Node,
        particle_system,
        base::AsBase,
        graph::Graph,
        camera::Camera,
    },
    core::math::{
        mat4::Mat4,
        vec3::Vec3,
        vec2::Vec2,
        Rect,
    },
};
use crate::renderer::TextureCache;

struct ParticleSystemShader {
    program: GpuProgram,
    view_projection_matrix: UniformLocation,
    world_matrix: UniformLocation,
    camera_side_vector: UniformLocation,
    camera_up_vector: UniformLocation,
    diffuse_texture: UniformLocation,
    depth_buffer_texture: UniformLocation,
    inv_screen_size: UniformLocation,
    proj_params: UniformLocation,
}

impl ParticleSystemShader {
    fn new() -> Result<Self, RendererError> {
        let vertex_source = include_str!("shaders/particle_system_vs.glsl");
        let fragment_source = include_str!("shaders/particle_system_fs.glsl");
        let mut program = GpuProgram::from_source("ParticleSystemShader", vertex_source, fragment_source)?;
        Ok(Self {
            view_projection_matrix: program.get_uniform_location("viewProjectionMatrix")?,
            world_matrix: program.get_uniform_location("worldMatrix")?,
            camera_side_vector: program.get_uniform_location("cameraSideVector")?,
            camera_up_vector: program.get_uniform_location("cameraUpVector")?,
            diffuse_texture: program.get_uniform_location("diffuseTexture")?,
            depth_buffer_texture: program.get_uniform_location("depthBufferTexture")?,
            inv_screen_size: program.get_uniform_location("invScreenSize")?,
            proj_params: program.get_uniform_location("projParams")?,
            program,
        })
    }

    pub fn bind(&mut self) {
        self.program.bind();
    }

    pub fn set_view_projection_matrix(&mut self, mat: &Mat4) -> &mut Self {
        self.program.set_mat4(self.view_projection_matrix, mat);
        self
    }

    pub fn set_world_matrix(&mut self, mat: &Mat4) -> &mut Self {
        self.program.set_mat4(self.world_matrix, mat);
        self
    }

    pub fn set_camera_side_vector(&mut self, vec: &Vec3) -> &mut Self {
        self.program.set_vec3(self.camera_side_vector, vec);
        self
    }

    pub fn set_camera_up_vector(&mut self, vec: &Vec3) -> &mut Self {
        self.program.set_vec3(self.camera_up_vector, vec);
        self
    }

    pub fn set_diffuse_texture(&mut self, id: i32) -> &mut Self {
        self.program.set_int(self.diffuse_texture, id);
        self
    }

    pub fn set_depth_buffer_texture(&mut self, id: i32) -> &mut Self {
        self.program.set_int(self.depth_buffer_texture, id);
        self
    }

    pub fn set_inv_screen_size(&mut self, size: Vec2) -> &mut Self {
        self.program.set_vec2(self.inv_screen_size, size);
        self
    }

    pub fn set_proj_params(&mut self, far: f32, near: f32) -> &mut Self {
        let params = Vec2::new(far, near);
        self.program.set_vec2(self.proj_params, params);
        self
    }
}

pub struct ParticleSystemRenderer {
    shader: ParticleSystemShader,
    draw_data: particle_system::DrawData,
    geometry_buffer: GeometryBuffer<particle_system::Vertex>,
    sorted_particles: Vec<u32>,
}

impl ParticleSystemRenderer {
    pub fn new() -> Result<Self, RendererError> {
        let geometry_buffer = GeometryBuffer::new(GeometryBufferKind::DynamicDraw, ElementKind::Triangle);

        geometry_buffer.describe_attributes(vec![
            AttributeDefinition { kind: AttributeKind::Float3, normalized: false },
            AttributeDefinition { kind: AttributeKind::Float2, normalized: false },
            AttributeDefinition { kind: AttributeKind::Float, normalized: false },
            AttributeDefinition { kind: AttributeKind::Float, normalized: false },
            AttributeDefinition { kind: AttributeKind::UnsignedByte4, normalized: true },
        ])?;

        Ok(Self {
            shader: ParticleSystemShader::new()?,
            draw_data: Default::default(),
            geometry_buffer,
            sorted_particles: Vec::new(),
        })
    }

    #[must_use]
    pub fn render(&mut self,
                  graph: &Graph,
                  camera: &Camera,
                  white_dummy: &GpuTexture,
                  frame_width: f32,
                  frame_height: f32,
                  gbuffer: &GBuffer,
                  gl_state: &mut GlState,
                  texture_cache: &mut TextureCache,
    ) -> RenderPassStatistics {
        let mut statistics = RenderPassStatistics::default();

        gl_state.push_viewport(Rect::new(0, 0, gbuffer.width, gbuffer.height));

        unsafe {
            gl::Disable(gl::CULL_FACE);
            gl::Enable(gl::BLEND);
            gl::DepthMask(gl::FALSE);
            gl::BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
        }

        self.shader.bind();

        let inv_view = camera.inv_view_matrix().unwrap();

        let camera_up = inv_view.up();
        let camera_side = inv_view.side();

        for node in graph.linear_iter() {
            let particle_system = if let Node::ParticleSystem(particle_system) = node {
                particle_system
            } else {
                continue;
            };

            particle_system.generate_draw_data(&mut self.sorted_particles,
                                               &mut self.draw_data,
                                               &camera.base().global_position());

            self.geometry_buffer.set_triangles(self.draw_data.get_triangles());
            self.geometry_buffer.set_vertices(self.draw_data.get_vertices());

            if let Some(texture) = particle_system.texture() {
                if let Some(texture) = texture_cache.get(texture) {
                    texture.bind(0);
                }
            } else {
                white_dummy.bind(0)
            }

            unsafe {
                gl::ActiveTexture(gl::TEXTURE1);
                gl::BindTexture(gl::TEXTURE_2D, gbuffer.depth_texture);
            }

            self.shader.set_diffuse_texture(0);
            self.shader.set_view_projection_matrix(&camera.view_projection_matrix());
            self.shader.set_world_matrix(&node.base().global_transform());
            self.shader.set_camera_up_vector(&camera_up);
            self.shader.set_camera_side_vector(&camera_side);
            self.shader.set_depth_buffer_texture(1);
            self.shader.set_inv_screen_size(Vec2::new(1.0 / frame_width, 1.0 / frame_height));
            self.shader.set_proj_params(camera.z_far(), camera.z_near());

            statistics.add_draw_call(self.geometry_buffer.draw());
        }

        unsafe {
            gl::Disable(gl::BLEND);
            gl::DepthMask(gl::TRUE);
        }

        gl_state.pop_viewport();

        statistics
    }
}
