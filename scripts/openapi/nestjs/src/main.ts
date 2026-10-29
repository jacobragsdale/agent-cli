// The orders API on NestJS with @nestjs/swagger set up as its docs show:
// SwaggerModule.setup('api', ...).
import 'reflect-metadata';
import {
  Body, Controller, Get, Module, NotFoundException, Param, ParseIntPipe, Post, Query,
} from '@nestjs/common';
import { NestFactory } from '@nestjs/core';
import {
  ApiCreatedResponse, ApiOkResponse, ApiProperty, ApiPropertyOptional, ApiQuery,
  DocumentBuilder, SwaggerModule,
} from '@nestjs/swagger';

class Line {
  @ApiProperty() sku: string;
  @ApiProperty() qty: number;
}

class OrderIn {
  @ApiProperty() customer_id: number;
  @ApiProperty({ type: [Line] }) lines: Line[];
  @ApiPropertyOptional() note?: string;
}

class Order extends OrderIn {
  @ApiProperty() id: number;
  @ApiProperty({ enum: ['open', 'shipped', 'cancelled', 'on_hold'] }) status: string;
}

const orders: Order[] = [1, 2, 3].map((i) => ({
  id: i, customer_id: 100 + i, status: 'open', lines: [{ sku: `SKU-00${i}`, qty: i }],
}));

@Controller('orders')
class OrdersController {
  @Get()
  @ApiQuery({ name: 'status', required: false })
  @ApiOkResponse({ type: [Order] })
  list(@Query('status') status?: string): Order[] {
    return orders.filter((o) => !status || o.status === status);
  }

  @Get(':id')
  @ApiOkResponse({ type: Order })
  get(@Param('id', ParseIntPipe) id: number): Order {
    const order = orders.find((o) => o.id === id);
    if (!order) throw new NotFoundException(`order ${id} not found`);
    return order;
  }

  @Post()
  @ApiCreatedResponse({ type: Order })
  create(@Body() body: OrderIn): Order {
    const order = { ...body, id: orders.length + 1, status: 'open' };
    orders.push(order);
    return order;
  }
}

@Module({ controllers: [OrdersController] })
class AppModule {}

async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  const config = new DocumentBuilder().setTitle('Contoso Orders API').setVersion('1.0').build();
  const documentFactory = () => SwaggerModule.createDocument(app, config);
  SwaggerModule.setup('api', app, documentFactory);
  await app.listen(3000);
}
bootstrap();
