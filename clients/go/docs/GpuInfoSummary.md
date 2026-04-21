# GpuInfoSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**MemoryMb** | **int64** | VRAM in MB | 
**Model** | **string** | Model name (e.g., \&quot;NVIDIA A100-SXM4-80GB\&quot;) | 
**Vendor** | **string** | Vendor: \&quot;nvidia\&quot;, \&quot;amd\&quot;, \&quot;intel\&quot;, or \&quot;unknown\&quot; | 

## Methods

### NewGpuInfoSummary

`func NewGpuInfoSummary(memoryMb int64, model string, vendor string, ) *GpuInfoSummary`

NewGpuInfoSummary instantiates a new GpuInfoSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGpuInfoSummaryWithDefaults

`func NewGpuInfoSummaryWithDefaults() *GpuInfoSummary`

NewGpuInfoSummaryWithDefaults instantiates a new GpuInfoSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMemoryMb

`func (o *GpuInfoSummary) GetMemoryMb() int64`

GetMemoryMb returns the MemoryMb field if non-nil, zero value otherwise.

### GetMemoryMbOk

`func (o *GpuInfoSummary) GetMemoryMbOk() (*int64, bool)`

GetMemoryMbOk returns a tuple with the MemoryMb field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryMb

`func (o *GpuInfoSummary) SetMemoryMb(v int64)`

SetMemoryMb sets MemoryMb field to given value.


### GetModel

`func (o *GpuInfoSummary) GetModel() string`

GetModel returns the Model field if non-nil, zero value otherwise.

### GetModelOk

`func (o *GpuInfoSummary) GetModelOk() (*string, bool)`

GetModelOk returns a tuple with the Model field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetModel

`func (o *GpuInfoSummary) SetModel(v string)`

SetModel sets Model field to given value.


### GetVendor

`func (o *GpuInfoSummary) GetVendor() string`

GetVendor returns the Vendor field if non-nil, zero value otherwise.

### GetVendorOk

`func (o *GpuInfoSummary) GetVendorOk() (*string, bool)`

GetVendorOk returns a tuple with the Vendor field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVendor

`func (o *GpuInfoSummary) SetVendor(v string)`

SetVendor sets Vendor field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


